// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The transition table, checked cell by cell against a table written here independently, and the
//! invariants of the directive's state machine held over every reachable order.

use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side};

use crate::fill::{Fill, Liquidity};
use crate::order::{Applied, LifecycleError, Order, OrderEvent, OrderState};

pub(crate) fn request(quantity_scaled: i128) -> OrderRequest {
    OrderRequest {
        client_id: "bl-1".into(),
        instrument: "AAA".into(),
        side: Side::Buy,
        quantity_scaled,
        price_scaled: Some(10_000),
        reduce_only: false,
        terms: Default::default(),
    }
}

pub(crate) fn fill(id: &str, quantity_scaled: i128) -> Fill {
    Fill {
        fill_id: id.into(),
        venue_order_id: Some("v-1".into()),
        side: Side::Buy,
        price_scaled: 10_000,
        quantity_scaled,
        fee: None,
        liquidity: Liquidity::Taker,
        exchange_time: ExchangeTime::At { at_ns: 5 },
        received_at_ns: 6,
    }
}

fn step(order: &Order, event: OrderEvent) -> Order {
    match order.apply(&event, 1) {
        Ok(Applied::Transitioned { order, .. }) => *order,
        other => panic!("{event:?} on {:?}: {other:?}", order.state),
    }
}

/// An order of ten, brought to `state` by the shortest valid path.
fn in_state(state: OrderState) -> Order {
    use OrderEvent as E;
    let created = Order::new(request(10));
    let submitted = || step(&created, E::Submitted);
    let accepted = || step(&submitted(), E::Accepted { venue_order_id: Some("v-1".into()) });
    let unknown = || step(&submitted(), E::OutcomeUnknown { detail: "timeout".into() });
    match state {
        OrderState::Created => created.clone(),
        OrderState::Submitted => submitted(),
        OrderState::Accepted => accepted(),
        OrderState::PartiallyFilled => step(&accepted(), E::Filled(fill("f-0", 4))),
        OrderState::CancelPending => step(&accepted(), E::CancelRequested),
        OrderState::Filled => step(&accepted(), E::Filled(fill("f-0", 10))),
        OrderState::Rejected => step(&submitted(), E::Rejected { reason: "below minimum".into() }),
        OrderState::Canceled => step(&accepted(), E::Canceled),
        OrderState::Expired => step(&accepted(), E::Expired),
        OrderState::NotSent => step(&created, E::NotSent),
        OrderState::Unknown => unknown(),
        OrderState::Reconciling => step(&unknown(), E::ReconciliationStarted),
    }
}

const STATES: [OrderState; 12] = [
    OrderState::Created,
    OrderState::Submitted,
    OrderState::Accepted,
    OrderState::PartiallyFilled,
    OrderState::CancelPending,
    OrderState::Filled,
    OrderState::Rejected,
    OrderState::Canceled,
    OrderState::Expired,
    OrderState::NotSent,
    OrderState::Unknown,
    OrderState::Reconciling,
];

fn events() -> [OrderEvent; 11] {
    [
        OrderEvent::Submitted,
        OrderEvent::NotSent,
        OrderEvent::Accepted { venue_order_id: Some("v-1".into()) },
        OrderEvent::Rejected { reason: "r".into() },
        OrderEvent::Filled(fill("f-new", 3)),
        OrderEvent::CancelRequested,
        OrderEvent::CancelRejected { reason: "r".into() },
        OrderEvent::Canceled,
        OrderEvent::Expired,
        OrderEvent::OutcomeUnknown { detail: "d".into() },
        OrderEvent::ReconciliationStarted,
    ]
}

/// Columns: submitted, not sent, accepted, rejected, fill of 3, cancel requested, cancel
/// rejected, canceled, expired, outcome unknown, reconciliation started. `None` is a refusal.
fn expected(state: OrderState) -> [Option<OrderState>; 11] {
    use OrderState::*;
    match state {
        Created => {
            [Some(Submitted), Some(NotSent), None, None, None, None, None, None, None, None, None]
        }
        Submitted => [
            None,
            Some(NotSent),
            Some(Accepted),
            Some(Rejected),
            Some(PartiallyFilled),
            None,
            None,
            Some(Canceled),
            Some(Expired),
            Some(Unknown),
            None,
        ],
        Accepted => [
            None,
            None,
            Some(Accepted),
            None,
            Some(PartiallyFilled),
            Some(CancelPending),
            None,
            Some(Canceled),
            Some(Expired),
            None,
            None,
        ],
        PartiallyFilled => [
            None,
            None,
            Some(PartiallyFilled),
            None,
            Some(PartiallyFilled),
            Some(CancelPending),
            None,
            Some(Canceled),
            Some(Expired),
            None,
            None,
        ],
        CancelPending => [
            None,
            None,
            None,
            None,
            Some(CancelPending),
            Some(CancelPending),
            Some(Accepted),
            Some(Canceled),
            Some(Expired),
            None,
            None,
        ],
        Filled | Rejected | NotSent => [None; 11],
        Canceled => [None, None, None, None, Some(Canceled), None, None, None, None, None, None],
        Expired => [None, None, None, None, Some(Expired), None, None, None, None, None, None],
        Unknown => [
            None,
            None,
            Some(Accepted),
            Some(Rejected),
            Some(PartiallyFilled),
            Some(Unknown),
            Some(Unknown),
            Some(Canceled),
            Some(Expired),
            None,
            Some(Reconciling),
        ],
        Reconciling => [
            None,
            None,
            Some(Accepted),
            Some(Rejected),
            Some(PartiallyFilled),
            Some(Reconciling),
            Some(Reconciling),
            Some(Canceled),
            Some(Expired),
            None,
            None,
        ],
    }
}

#[test]
fn every_cell_of_the_transition_table_is_as_declared() {
    let mut checked = 0;
    for state in STATES {
        let order = in_state(state);
        assert_eq!(order.state, state, "the path to {state:?} arrived elsewhere");
        for (event, want) in events().iter().zip(expected(state)) {
            let got = match order.apply(event, 9) {
                Ok(Applied::Transitioned { order, .. }) => Some(order.state),
                Ok(Applied::Duplicate) => panic!("{event:?} on {state:?} read as a duplicate"),
                Err(_) => None,
            };
            assert_eq!(got, want, "{} on {}", event.name(), state.name());
            checked += 1;
        }
    }
    assert_eq!(checked, 132);
}

/// The audit record carries the directive's five fields.
#[test]
fn a_transition_records_from_to_cause_venue_identifier_and_time() {
    let Ok(Applied::Transitioned { transition, .. }) = in_state(OrderState::Submitted)
        .apply(&OrderEvent::Accepted { venue_order_id: Some("v-9".into()) }, 77)
    else {
        panic!("not applied");
    };
    assert_eq!((transition.from, transition.to), (OrderState::Submitted, OrderState::Accepted));
    assert_eq!(transition.cause, "order_acked");
    assert_eq!(transition.venue_order_id.as_deref(), Some("v-9"));
    assert_eq!((transition.at_ns, transition.client_id.as_str()), (77, "bl-1"));
}

#[test]
fn unknown_is_neither_terminal_nor_safe_to_forget() {
    for state in [OrderState::Unknown, OrderState::Reconciling] {
        assert!(!state.is_terminal());
        assert!(state.may_execute());
    }
}

#[test]
fn a_fill_beyond_the_ordered_quantity_is_refused_and_changes_nothing() {
    let order = in_state(OrderState::PartiallyFilled);
    let refused = order.apply(&OrderEvent::Filled(fill("f-9", 7)), 2);
    assert!(matches!(
        refused,
        Err(LifecycleError::Overfill { ordered: 10, filled: 4, offered: 7, .. })
    ));
    assert_eq!(order.filled_quantity_scaled, 4);
}

#[test]
fn a_fill_delivered_twice_is_a_duplicate_and_a_reused_identity_is_a_conflict() {
    let order = in_state(OrderState::PartiallyFilled);
    let mut again = fill("f-0", 4);
    again.received_at_ns = 999;
    assert_eq!(order.apply(&OrderEvent::Filled(again), 2), Ok(Applied::Duplicate));

    let conflicting = fill("f-0", 5);
    assert!(matches!(
        order.apply(&OrderEvent::Filled(conflicting), 2),
        Err(LifecycleError::ConflictingFill { .. })
    ));
}

#[test]
fn a_fill_that_contradicts_its_order_is_refused() {
    let order = in_state(OrderState::Accepted);
    let mut sold = fill("f-1", 1);
    sold.side = Side::Sell;
    let mut elsewhere = fill("f-2", 1);
    elsewhere.venue_order_id = Some("v-other".into());
    let mut anonymous = fill("", 1);
    anonymous.venue_order_id = None;
    for bad in [sold, elsewhere, anonymous] {
        assert!(matches!(
            order.apply(&OrderEvent::Filled(bad), 2),
            Err(LifecycleError::Contradiction { .. })
        ));
    }
}

/// A cancel can cross a fill: the fill that completes the order wins, and a late partial fill is
/// still counted on a canceled order.
#[test]
fn fills_that_cross_a_cancel_are_counted() {
    let pending = in_state(OrderState::CancelPending);
    assert_eq!(step(&pending, OrderEvent::Filled(fill("f-1", 10))).state, OrderState::Filled);

    let canceled = in_state(OrderState::Canceled);
    let late = step(&canceled, OrderEvent::Filled(fill("f-1", 3)));
    assert_eq!((late.state, late.filled_quantity_scaled), (OrderState::Canceled, 3));
}

#[test]
fn a_rejection_after_a_fill_is_refused() {
    let reconciling = in_state(OrderState::Reconciling);
    let partly = step(&reconciling, OrderEvent::Filled(fill("f-1", 2)));
    let back = Order { state: OrderState::Reconciling, ..partly };
    assert!(back.apply(&OrderEvent::Rejected { reason: "r".into() }, 3).is_err());
}

/// Every reachable order, along every sequence of up to seven events, keeps the invariants:
/// filled equals the sum of its fills, never exceeds the order, and `Filled` has a fill.
#[test]
fn the_invariants_hold_over_every_reachable_order() {
    let mut frontier = vec![Order::new(request(10))];
    let mut visited = 0;
    for depth in 0..7 {
        let mut next = Vec::new();
        for order in &frontier {
            for (i, event) in events().into_iter().enumerate() {
                let event = match event {
                    OrderEvent::Filled(_) => OrderEvent::Filled(fill(&format!("f-{depth}-{i}"), 3)),
                    other => other,
                };
                if let Ok(Applied::Transitioned { order, .. }) = order.apply(&event, depth) {
                    let sum: i128 = order.fills.iter().map(|f| f.quantity_scaled).sum();
                    assert_eq!(sum, order.filled_quantity_scaled);
                    assert!(order.filled_quantity_scaled <= order.request.quantity_scaled);
                    if order.state == OrderState::Filled {
                        assert!(!order.fills.is_empty());
                        assert_eq!(order.remaining_scaled(), 0);
                    }
                    visited += 1;
                    next.push(*order);
                }
            }
        }
        frontier = next;
    }
    assert!(visited > 1_000, "the search covered {visited} orders");
}
