// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Linked proposals folded from the record (decision 0013): what is held, released and cancelled.

use bijective_authority::{Action, Role};
use bijective_event_store::projection::Projection;
use bijective_lifecycle::{Fill, Liquidity};
use bijective_limits::SafeState;
use bijective_model::market::ExchangeTime;
use bijective_model::{InstrumentId, OrderRequest, Proposal, Side};
use bijective_venue::normalise::{Rejection, VenueOutcome};

use crate::replay::Replayed;
use crate::wire::ControlEvent;

fn proposal(id: &str, side: Side, quantity: i128) -> Proposal {
    Proposal {
        desk: "alpha".into(),
        instrument: InstrumentId::new("aaa"),
        side,
        quantity_scaled: quantity,
        price_scaled: Some(200),
        cap_scaled: None,
        client_id: id.into(),
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: false,
        parent: None,
        oco: None,
    }
}

/// A reduce-only child of `entry` in the bracket group.
fn child(id: &str) -> Proposal {
    Proposal {
        reduce_only: true,
        parent: Some("entry".into()),
        oco: Some("bracket".into()),
        ..proposal(id, Side::Sell, 10)
    }
}

fn held(proposal: Proposal, from_position: u64) -> ControlEvent {
    ControlEvent::Held { proposal, from_position, at_ns: 1 }
}

/// An intent for `decision` under `client_id`, and the venue's acceptance.
fn placed(decision: &str, client_id: &str, side: Side, quantity: i128) -> [ControlEvent; 2] {
    let request = OrderRequest {
        client_id: client_id.into(),
        instrument: "aaa".into(),
        side,
        quantity_scaled: quantity,
        price_scaled: Some(200),
        reduce_only: side == Side::Sell,
        terms: Default::default(),
    };
    [
        ControlEvent::Intent {
            decision_id: decision.into(),
            venue: Some("venue-a".into()),
            owner: None,
            request,
            from_position: 0,
            at_ns: 2,
        },
        ControlEvent::Settled {
            client_id: client_id.into(),
            outcome: VenueOutcome::Accepted,
            at_ns: 3,
        },
    ]
}

fn filled(client_id: &str, fill_id: &str, side: Side, quantity: i128) -> ControlEvent {
    let fill = Fill {
        fill_id: fill_id.into(),
        venue_order_id: None,
        side,
        price_scaled: 200,
        quantity_scaled: quantity,
        fee: None,
        liquidity: Liquidity::Taker,
        exchange_time: ExchangeTime::At { at_ns: 4 },
        received_at_ns: 4,
    };
    ControlEvent::Filled { client_id: client_id.into(), fill, at_ns: 4 }
}

fn fold(events: &[ControlEvent]) -> Replayed {
    let mut replayed = Replayed::default();
    for e in events {
        replayed.apply(e).expect("applied");
    }
    replayed
}

/// What is due, by identifier and quantity.
fn due(replayed: &Replayed) -> Vec<(String, i128)> {
    replayed
        .linked()
        .releasable(replayed.orders())
        .into_iter()
        .map(|r| (r.proposal.client_id, r.proposal.quantity_scaled))
        .collect()
}

#[test]
fn a_child_is_not_due_before_its_parent_fills() {
    let mut events = vec![held(child("tp"), 1)];
    events.extend(placed("entry", "o-entry", Side::Buy, 10));
    assert!(due(&fold(&events)).is_empty());
}

#[test]
fn a_child_is_due_what_its_parent_filled_and_no_more_than_it_asked() {
    let mut events = vec![held(child("tp"), 1)];
    events.extend(placed("entry", "o-entry", Side::Buy, 20));
    events.push(filled("o-entry", "f1", Side::Buy, 4));
    assert_eq!(due(&fold(&events)), vec![("tp".to_string(), 4)]);

    events.extend(placed("tp", "o-tp-1", Side::Sell, 4));
    assert!(due(&fold(&events)).is_empty(), "released once, by its intent");

    events.push(filled("o-entry", "f2", Side::Buy, 16));
    assert_eq!(due(&fold(&events)), vec![("tp".to_string(), 6)], "capped at the 10 it asked");
}

#[test]
fn a_parent_that_ends_without_a_fill_releases_nothing() {
    let mut events = vec![held(child("tp"), 1)];
    events.push(ControlEvent::Intent {
        decision_id: "entry".into(),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: "o-entry".into(),
            instrument: "aaa".into(),
            side: Side::Buy,
            quantity_scaled: 10,
            price_scaled: Some(200),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position: 0,
        at_ns: 2,
    });
    events.push(ControlEvent::Settled {
        client_id: "o-entry".into(),
        outcome: VenueOutcome::Rejected(Rejection::BelowMinimumSize),
        at_ns: 3,
    });
    assert!(due(&fold(&events)).is_empty());
}

#[test]
fn a_fill_on_one_leg_cancels_the_other_and_ends_its_releases() {
    let mut events = vec![held(child("tp"), 1), held(child("sl"), 2)];
    events.extend(placed("entry", "o-entry", Side::Buy, 20));
    events.push(filled("o-entry", "f1", Side::Buy, 10));
    events.extend(placed("tp", "o-tp", Side::Sell, 10));
    events.extend(placed("sl", "o-sl", Side::Sell, 10));
    let before = fold(&events);
    assert!(before.linked().displaced_orders(before.orders()).is_empty());
    assert_eq!(before.linked().peer_orders("tp"), ["o-sl".to_string()].into());

    events.push(filled("o-tp", "f2", Side::Sell, 3));
    let after = fold(&events);
    assert_eq!(after.linked().displaced_orders(after.orders()), ["o-sl".to_string()].into());

    events.push(filled("o-entry", "f3", Side::Buy, 10));
    assert!(due(&fold(&events)).is_empty(), "the filled leg asked 10 and has 10");
}

#[test]
fn a_new_member_of_a_group_that_filled_is_seen_as_such() {
    let mut events = vec![held(child("tp"), 1)];
    events.extend(placed("entry", "o-entry", Side::Buy, 10));
    events.push(filled("o-entry", "f1", Side::Buy, 10));
    events.extend(placed("tp", "o-tp", Side::Sell, 10));
    events.push(filled("o-tp", "f2", Side::Sell, 10));
    let replayed = fold(&events);
    assert!(replayed.linked().group_filled(&child("sl"), replayed.orders()));
    let other = Proposal { parent: Some("other".into()), ..child("sl") };
    assert!(!replayed.linked().group_filled(&other, replayed.orders()), "scoped to its parent");
}

#[test]
fn a_cancel_orders_command_ends_what_is_still_held() {
    let mut events = vec![held(child("tp"), 1)];
    events.extend(placed("entry", "o-entry", Side::Buy, 10));
    events.push(ControlEvent::CommandApplied {
        command_id: "c1".into(),
        actor: "ops".into(),
        role: Role::RiskOfficer,
        action: Action::CancelOrders,
        state: SafeState::Active,
        detail: String::new(),
        from_position: 0,
        at_ns: 5,
    });
    events.push(filled("o-entry", "f1", Side::Buy, 10));
    assert!(due(&fold(&events)).is_empty());
}

/// Each holding's identifier, released quantity and state.
fn standing(replayed: &Replayed) -> Vec<(String, i128, crate::linked::HoldState)> {
    let holdings = replayed.linked().holdings(replayed.orders());
    holdings.into_iter().map(|h| (h.proposal.client_id, h.released_scaled, h.state)).collect()
}

#[test]
fn a_holding_reads_waiting_released_displaced_or_ended() {
    use crate::linked::HoldState::{Displaced, Ended, Released, Waiting};
    let mut events = vec![held(child("sl"), 1), held(child("tp"), 2)];
    events.extend(placed("entry", "o-entry", Side::Buy, 10));
    let waiting = vec![("sl".to_string(), 0, Waiting), ("tp".to_string(), 0, Waiting)];
    assert_eq!(standing(&fold(&events)), waiting);

    events.push(filled("o-entry", "f1", Side::Buy, 10));
    events.extend(placed("tp", "o-tp", Side::Sell, 10));
    events.extend(placed("sl", "o-sl", Side::Sell, 4));
    events.push(filled("o-tp", "f2", Side::Sell, 10));
    let after = vec![("sl".to_string(), 4, Displaced), ("tp".to_string(), 10, Released)];
    assert_eq!(standing(&fold(&events)), after);

    let mut cancelled = vec![held(child("tp"), 1)];
    cancelled.push(ControlEvent::CommandApplied {
        command_id: "c1".into(),
        actor: "ops".into(),
        role: Role::RiskOfficer,
        action: Action::Flatten,
        state: SafeState::Active,
        detail: String::new(),
        from_position: 0,
        at_ns: 5,
    });
    assert_eq!(standing(&fold(&cancelled)), vec![("tp".to_string(), 0, Ended)]);
}

#[test]
fn a_proposal_with_a_group_and_no_parent_is_due_at_once() {
    let alone =
        Proposal { oco: Some("exit".into()), reduce_only: true, ..proposal("x", Side::Sell, 5) };
    assert_eq!(due(&fold(&[held(alone, 1)])), vec![("x".to_string(), 5)]);
}
