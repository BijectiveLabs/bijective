// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a flatten closes, and how long the command stays to be acted on.

use bijective_authority::{Action, Role};
use bijective_event_store::projection::Projection;
use bijective_limits::SafeState;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::VenueOutcome;

use super::{Closing, closing_orders};
use crate::orders::Orders;
use crate::progress::Progress;
use crate::wire::{ControlEvent, EvidenceRead, flatten_decision_id};

fn request(client_id: &str, side: Side, quantity_scaled: i128, reduce_only: bool) -> OrderRequest {
    OrderRequest {
        client_id: client_id.into(),
        instrument: "AAA".into(),
        side,
        quantity_scaled,
        price_scaled: Some(100),
        reduce_only,
        terms: Default::default(),
    }
}

fn placed(venue: &str, decision_id: &str, request: OrderRequest) -> [ControlEvent; 2] {
    let client_id = request.client_id.clone();
    [
        ControlEvent::Intent {
            decision_id: decision_id.into(),
            venue: Some(venue.into()),
            owner: None,
            request,
            from_position: 0,
            at_ns: 1,
        },
        ControlEvent::Settled { client_id, outcome: VenueOutcome::Accepted, at_ns: 2 },
    ]
}

/// A buy of 10 on `venue`, filled in full.
fn held_long(venue: &str, client_id: &str) -> Vec<ControlEvent> {
    let mut events = placed(venue, "dec-1", request(client_id, Side::Buy, 10, false)).to_vec();
    let fill = bijective_lifecycle::Fill {
        fill_id: format!("fill-{client_id}"),
        venue_order_id: Some("v".into()),
        side: Side::Buy,
        price_scaled: 100,
        quantity_scaled: 10,
        fee: None,
        liquidity: bijective_lifecycle::Liquidity::Maker,
        exchange_time: bijective_model::market::ExchangeTime::NotPublished,
        received_at_ns: 3,
    };
    events.push(ControlEvent::Filled { client_id: client_id.into(), fill, at_ns: 3 });
    events
}

fn command(action: Action, detail: &str) -> ControlEvent {
    ControlEvent::CommandApplied {
        command_id: "cmd-9".into(),
        actor: "ana".into(),
        role: Role::Operator,
        action,
        state: SafeState::Reducing,
        detail: detail.into(),
        from_position: 0,
        at_ns: 4,
    }
}

fn fold(events: impl IntoIterator<Item = ControlEvent>) -> Orders {
    let mut orders = Orders::default();
    for e in events {
        orders.apply(&e).expect("applied");
    }
    orders
}

#[test]
fn a_held_position_is_closed_by_its_whole_quantity_on_the_other_side() {
    let (closing, untrusted) = closing_orders(&fold(held_long("venue-a", "c-1")), &[]);
    assert_eq!(
        closing,
        vec![Closing {
            venue: "venue-a".into(),
            instrument: VenueSymbol::new("AAA"),
            side: Side::Sell,
            quantity_scaled: 10,
        }]
    );
    assert!(untrusted.is_empty());
}

/// A reduce-only exit already working is counted, so the two together do not pass through flat.
#[test]
fn a_working_reduce_only_exit_is_left_to_close_its_share() {
    let mut events = held_long("venue-a", "c-1");
    events.extend(placed("venue-a", "dec-2", request("c-2", Side::Sell, 4, true)));
    let (closing, _) = closing_orders(&fold(events), &[]);
    assert_eq!(closing[0].quantity_scaled, 6);
}

#[test]
fn a_venue_whose_last_read_failed_is_reported_not_closed_by_a_guess() {
    let mut events = held_long("venue-a", "c-1");
    events.push(ControlEvent::ReconciliationCompleted {
        venue: "venue-a".into(),
        verdict: bijective_lifecycle::Verdict::Unverified,
        evidence: EvidenceRead::Failed,
        findings: Vec::new(),
        at_ns: 5,
    });
    let (closing, untrusted) = closing_orders(&fold(events), &[]);
    assert!(closing.is_empty());
    assert_eq!(untrusted, vec!["venue-a".to_string()]);
}

/// A flatten stays in force after its orders, so a remainder an immediate order left is closed on
/// a later pass. It orders every open order cancelled.
#[test]
fn a_flatten_stays_in_force_and_orders_every_open_order_cancelled() {
    let mut events = placed("venue-a", "dec-2", request("c-2", Side::Sell, 4, true)).to_vec();
    events.push(command(Action::Flatten, ""));
    events.extend(placed(
        "venue-a",
        &flatten_decision_id("cmd-9"),
        request("c-3", Side::Sell, 10, true),
    ));
    let orders = fold(events);
    assert_eq!(orders.flatten_asked(), Some("cmd-9"));
    assert!(orders.is_ordered_cancelled("c-2"));
}

/// On a venue that enforces reduce-only, a stop ordered cancelled does not stand in for closing:
/// the whole position is closed at the market, and the venue cannot let both pass through flat.
#[test]
fn an_exit_ordered_cancelled_is_not_counted_where_the_venue_enforces_reduce_only() {
    let mut events = held_long("venue-a", "c-1");
    events.extend(placed("venue-a", "dec-2", request("c-2", Side::Sell, 4, true)));
    events.push(command(Action::Flatten, ""));
    let orders = fold(events);
    assert_eq!(closing_orders(&orders, &["venue-a".to_string()]).0[0].quantity_scaled, 10);
    assert_eq!(closing_orders(&orders, &[]).0[0].quantity_scaled, 6);
}

#[test]
fn a_refused_flatten_asks_nothing_and_a_resume_withdraws_one_asked() {
    assert_eq!(fold([command(Action::Flatten, "observer may not flatten")]).flatten_asked(), None);
    let resumed = fold([command(Action::Flatten, ""), command(Action::Resume, "")]);
    assert_eq!(resumed.flatten_asked(), None);
}

/// A flatten order names no proposal: counting it would skip a proposal not yet acted on.
#[test]
fn a_flatten_order_does_not_move_the_proposal_mark() {
    let mut progress = Progress::default();
    for e in placed("venue-a", &flatten_decision_id("cmd-9"), request("c-3", Side::Sell, 10, true))
    {
        progress.apply(&e).expect("applied");
    }
    assert_eq!(progress.resume_from(), 0);
}
