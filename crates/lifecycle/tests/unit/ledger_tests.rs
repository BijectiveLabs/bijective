// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The ledger moves an order and its position together, or neither.

use bijective_model::{Side, VenueSymbol};

use crate::ledger::{Ledger, LedgerChange, LedgerError};
use crate::order::{OrderEvent, OrderState};
use crate::order_tests::{fill, request};
use crate::position::PositionCause;

fn working() -> Ledger {
    let mut l = Ledger::default();
    l.create(request(10)).unwrap();
    l.apply("bl-1", &OrderEvent::Submitted, 1).unwrap();
    l.apply("bl-1", &OrderEvent::Accepted { venue_order_id: Some("v-1".into()) }, 2).unwrap();
    l
}

#[test]
fn a_fill_moves_the_order_and_the_position_together() {
    let mut l = working();
    let change = l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 4)), 3).unwrap();
    let LedgerChange::Applied { transition, position: Some(position) } = change else {
        panic!("{change:?}");
    };
    assert_eq!(transition.to, OrderState::PartiallyFilled);
    assert_eq!(position.quantity_after_scaled, 4);
    assert_eq!(l.position(&VenueSymbol::new("AAA")).unwrap().quantity_scaled(), 4);
}

#[test]
fn a_fill_delivered_twice_changes_the_position_once() {
    let mut l = working();
    l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 4)), 3).unwrap();
    assert_eq!(
        l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 4)), 4),
        Ok(LedgerChange::Duplicate)
    );
    assert_eq!(l.position(&VenueSymbol::new("AAA")).unwrap().quantity_scaled(), 4);
}

/// The position refuses an identity an adjustment already used. The order must not move either.
#[test]
fn when_the_position_refuses_the_order_does_not_move() {
    let mut l = working();
    let used = PositionCause::Adjustment { adjustment_id: "f-1".into(), reason: "transfer".into() };
    l.adjust(&VenueSymbol::new("AAA"), used, Side::Buy, 100, 1).unwrap();
    let before = l.clone();
    assert!(matches!(
        l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 4)), 3),
        Err(LedgerError::Position(_))
    ));
    assert_eq!(l, before);
}

#[test]
fn one_client_identifier_names_one_order() {
    let mut l = working();
    assert!(matches!(l.create(request(5)), Err(LedgerError::DuplicateClientId { .. })));
    assert!(matches!(
        l.apply("bl-x", &OrderEvent::Submitted, 1),
        Err(LedgerError::UnknownClientId { .. })
    ));
}

#[test]
fn the_summary_counts_what_may_still_execute_and_what_is_unknown() {
    let mut l = working();
    let mut second = request(3);
    second.client_id = "bl-2".into();
    l.create(second).unwrap();
    l.apply("bl-2", &OrderEvent::Submitted, 1).unwrap();
    l.apply("bl-2", &OrderEvent::OutcomeUnknown { detail: "timeout".into() }, 2).unwrap();
    l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 10)), 3).unwrap();

    let s = l.summary();
    assert_eq!((s.orders, s.may_execute, s.unknown, s.positions_open), (2, 1, 1, 1));
}

/// A fill whose fee the venue did not state moves the position and is counted, so the fee total
/// reads as a lower bound rather than the cost.
#[test]
fn an_unstated_fee_is_counted_not_read_as_zero() {
    let mut ledger = Ledger::default();
    ledger.create(crate::order_tests::request(10)).unwrap();
    ledger.apply("bl-1", &OrderEvent::Submitted, 1).unwrap();
    ledger.apply("bl-1", &OrderEvent::Accepted { venue_order_id: None }, 2).unwrap();
    let unstated = crate::order_tests::fill("f-1", 4);
    assert!(unstated.fee.is_none());
    ledger.apply("bl-1", &OrderEvent::Filled(unstated), 3).unwrap();
    let position = ledger.position(&bijective_model::VenueSymbol::new("AAA")).unwrap();
    assert_eq!((position.quantity_scaled(), position.fills_without_fee), (4, 1));
    assert!(position.fees.is_empty());
}
