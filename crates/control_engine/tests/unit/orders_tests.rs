// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The control stream folded into the order lifecycle.

use bijective_event_store::projection::Projection;
use bijective_lifecycle::OrderState;
use bijective_model::{OrderRequest, Side};
use bijective_venue::normalise::{Rejection, Unknown, VenueOutcome};

use super::Orders;
use crate::wire::ControlEvent;

fn intent(client_id: &str) -> ControlEvent {
    ControlEvent::Intent {
        decision_id: format!("dec-{client_id}"),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: client_id.into(),
            instrument: "AAA".into(),
            side: Side::Buy,
            quantity_scaled: 10,
            price_scaled: Some(100),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position: 0,
        at_ns: 1,
    }
}

fn settled(client_id: &str, outcome: VenueOutcome) -> ControlEvent {
    ControlEvent::Settled { client_id: client_id.into(), outcome, at_ns: 2 }
}

fn state(orders: &Orders, client_id: &str) -> OrderState {
    orders.order(client_id).expect("recorded").state
}

fn fold(events: &[ControlEvent]) -> Orders {
    let mut orders = Orders::default();
    for e in events {
        orders.apply(e).expect("applied");
    }
    orders
}

/// An intent with no settlement is an order in flight, not an order never sent: the crash it
/// records happened after the intent was written and may have been after the call.
#[test]
fn an_intent_without_its_answer_is_an_order_that_may_execute() {
    let orders = fold(&[intent("a")]);
    assert_eq!(state(&orders, "a"), OrderState::Submitted);
    assert_eq!(orders.summary().may_execute, 1);
}

#[test]
fn each_answer_moves_the_order_as_the_lifecycle_defines() {
    let orders = fold(&[
        intent("a"),
        settled("a", VenueOutcome::Accepted),
        intent("b"),
        settled("b", VenueOutcome::Rejected(Rejection::BelowMinimumSize)),
        intent("c"),
        settled("c", VenueOutcome::Unknown(Unknown::Timeout)),
        intent("d"),
        settled("d", VenueOutcome::NotSent),
        intent("e"),
        settled("e", VenueOutcome::Rejected(Rejection::Duplicate)),
    ]);
    assert_eq!(state(&orders, "a"), OrderState::Accepted);
    assert_eq!(state(&orders, "b"), OrderState::Rejected);
    assert_eq!(state(&orders, "c"), OrderState::Unknown);
    assert_eq!(state(&orders, "d"), OrderState::NotSent);
    assert_eq!(state(&orders, "e"), OrderState::Accepted, "a duplicate says the venue holds it");
    let s = orders.summary();
    assert_eq!((s.may_execute, s.unknown), (3, 1));
    assert!(orders.findings().is_empty());
}

#[test]
fn a_withdrawal_leaves_the_order_pending_until_the_venue_says_otherwise() {
    let withdrawn = |outcome| ControlEvent::Withdrawn {
        client_id: "a".into(),
        instrument: "AAA".into(),
        outcome,
        at_ns: 3,
    };
    let accepted = fold(&[
        intent("a"),
        settled("a", VenueOutcome::Accepted),
        withdrawn(VenueOutcome::Accepted),
    ]);
    assert_eq!(state(&accepted, "a"), OrderState::CancelPending);

    let refused = fold(&[
        intent("a"),
        settled("a", VenueOutcome::Accepted),
        withdrawn(VenueOutcome::Rejected(Rejection::Unclassified { venue_code: "x".into() })),
    ]);
    assert_eq!(state(&refused, "a"), OrderState::Accepted);
}

/// A record the state machine refuses is kept and counted, never dropped.
#[test]
fn a_contradiction_in_the_record_becomes_a_finding() {
    let orders = fold(&[
        intent("a"),
        intent("a"),
        settled("a", VenueOutcome::Rejected(Rejection::BelowMinimumSize)),
        settled("a", VenueOutcome::Accepted),
        settled("zz", VenueOutcome::Accepted),
    ]);
    assert_eq!(orders.findings().len(), 3, "{:?}", orders.findings());
    assert_eq!(state(&orders, "a"), OrderState::Rejected);
}

fn filled(client_id: &str, side: Side, price_scaled: i128, at_ns: i128) -> [ControlEvent; 2] {
    let mut placed = intent(client_id);
    if let ControlEvent::Intent { request, .. } = &mut placed {
        request.side = side;
    }
    let fill = bijective_lifecycle::Fill {
        fill_id: format!("fill-{client_id}"),
        venue_order_id: Some("v".into()),
        side,
        price_scaled,
        quantity_scaled: 10,
        fee: None,
        liquidity: bijective_lifecycle::Liquidity::Maker,
        exchange_time: bijective_model::market::ExchangeTime::NotPublished,
        received_at_ns: at_ns,
    };
    [placed, ControlEvent::Filled { client_id: client_id.into(), fill, at_ns }]
}

/// Only a fill that closes something realises profit, and each is kept with the time it was recorded.
#[test]
fn a_closing_fill_is_kept_as_realised_profit_with_its_time() {
    let mut events = Vec::new();
    events.extend(filled("open", Side::Buy, 100, 1_000));
    events.extend(filled("close", Side::Sell, 130, 5_000));
    let orders = fold(&events);

    let all: Vec<_> = orders.realized_since(0).collect();
    assert_eq!(all.len(), 1, "the opening fill realised nothing");
    assert_eq!((all[0].realized_scaled, all[0].at_ns), (300, 5_000));
    assert_eq!(all[0].venue, "venue-a");
    assert_eq!(orders.realized_since(5_000).count(), 1);
    assert_eq!(orders.realized_since(5_001).count(), 0, "a fill before the day is not today's");
}

/// Opening fills pay fees too, and a fill delivered twice is charged once.
#[test]
fn every_applied_fill_is_charged_once_whatever_it_realised() {
    let [placed, fill] = filled("open", Side::Buy, 100, 5);
    let orders = fold(&[placed, fill.clone(), fill]);
    let charged: Vec<_> = orders.charged_since(0).collect();
    assert_eq!(charged.len(), 1, "{charged:?}");
    assert_eq!(charged[0].fee, None, "an unstated fee is carried as unstated");
    assert_eq!(orders.realized_since(0).count(), 0, "an opening fill realises nothing");
    assert_eq!(orders.charged_since(6).count(), 0);
}

/// Who each order was placed for comes from its intent. What it could still fill is counted
/// until the order is terminal, and a sell is signed short.
#[test]
fn every_placed_order_carries_its_owner_its_fills_and_what_it_could_still_fill() {
    let owner = bijective_model::Owner { desk: "alpha".into(), strategy: Some("trend".into()) };
    let mut owned = intent("bl-owned");
    if let ControlEvent::Intent { owner: recorded, .. } = &mut owned {
        *recorded = Some(owner.clone());
    }
    let [sold, fill] = filled("bl-sold", Side::Sell, 100, 3);
    let orders = fold(&[owned, settled("bl-owned", VenueOutcome::Accepted), sold, fill]);

    let placed = orders
        .placed(|_, symbol| Some(bijective_model::InstrumentId::new(symbol.as_str())))
        .expect("every symbol named");

    let working = placed.iter().find(|p| p.owner.is_some()).expect("the owned order");
    let done = placed.iter().find(|p| p.owner.is_none()).expect("the unowned order");
    assert_eq!(working.owner.as_ref(), Some(&owner));
    assert_eq!((working.open_buy_scaled, working.open_sell_scaled), (10, 0));
    assert_eq!(done.owner, None, "an intent without an owner is nobody's, never a default desk's");
    assert_eq!(done.filled_scaled, -10);
    assert_eq!(
        (done.open_buy_scaled, done.open_sell_scaled),
        (0, 0),
        "a filled order adds nothing"
    );
}

/// A symbol the venue's catalogue cannot name is an order nobody's figure could hold.
#[test]
fn an_order_on_a_symbol_without_an_identity_is_named_rather_than_left_out() {
    let orders = fold(&[intent("bl-1")]);

    let refused = orders.placed(|_, _| None).expect_err("an unnamed symbol");
    assert_eq!(refused.0, "venue-a");
}

/// An adjustment that named its owner is part of that owner's exposure, as a fill would be. One
/// that named none is left to the residue every desk is charged.
#[test]
fn an_owned_adjustment_is_placed_as_its_owner_s_filled_quantity() {
    let adjusted = |id: &str, owner| ControlEvent::PositionAdjusted {
        adjustment: crate::wire::Adjustment {
            venue: "venue-a".into(),
            adjustment_id: id.into(),
            actor: "risk".into(),
            instrument: "AAA".into(),
            side: Side::Sell,
            quantity_scaled: 4,
            price_scaled: 100,
            reason: "moved in".into(),
            owner,
        },
        at_ns: 1,
    };
    let owner = bijective_model::Owner { desk: "alpha".into(), strategy: None };
    let orders = fold(&[adjusted("adj-1", Some(owner.clone())), adjusted("adj-2", None)]);

    let placed = orders
        .placed(|_, symbol| Some(bijective_model::InstrumentId::new(symbol.as_str())))
        .expect("every symbol named");
    assert_eq!(placed.len(), 1, "{placed:?}");
    assert_eq!(placed[0].owner.as_ref(), Some(&owner));
    assert_eq!(placed[0].filled_scaled, -4);
}

/// A venue adapter takes its orders back with the instant each intent was recorded, because a
/// venue that knows an order by its signed hash signs that instant into it.
#[test]
fn each_venue_s_orders_are_handed_back_with_their_intent_instant_and_whether_they_finished() {
    let mut other = intent("c");
    if let ControlEvent::Intent { venue, at_ns, .. } = &mut other {
        *venue = Some("venue-b".into());
        *at_ns = 7;
    }
    let orders = fold(&[
        intent("a"),
        settled("a", VenueOutcome::Rejected(Rejection::BelowMinimumSize)),
        intent("b"),
        other,
    ]);

    let recorded = orders.recorded_for("venue-a");
    let summary: Vec<(&str, i128, bool)> =
        recorded.iter().map(|r| (r.request.client_id.as_str(), r.intent_ns, r.terminal)).collect();
    assert_eq!(summary, vec![("a", 1, true), ("b", 1, false)]);
    assert_eq!(orders.recorded_for("venue-b")[0].intent_ns, 7);
    assert!(orders.recorded_for("venue-z").is_empty());
}
