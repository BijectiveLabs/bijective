// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Acceptance tests for the desk register.

use super::*;

fn amount(v: i128) -> Amount {
    Amount::from_scaled(v)
}

fn entity(desk: &str) -> Entity {
    Entity {
        desk: desk.to_string(),
        venue: "a-venue".into(),
        strategy: "a-strategy".into(),
        instrument: "an-instrument".into(),
    }
}

fn two_desks() -> Register {
    Register::new(
        amount(1_000_000),
        [("macro".to_string(), amount(600_000)), ("relative-value".to_string(), amount(300_000))],
    )
    .unwrap()
}

#[test]
fn a_limit_against_an_unregistered_desk_is_refused() {
    let register = two_desks();

    assert_eq!(
        register.admits(&Scope::Desk("marco".into())).unwrap_err(),
        RegisterError::UnknownDesk { name: "marco".into() },
        "a typo must not create a desk"
    );
    assert!(register.admits(&Scope::Desk("macro".into())).is_ok());
}

#[test]
fn the_register_constrains_the_desk_level_and_no_other() {
    let register = two_desks();

    assert!(register.admits(&Scope::Portfolio).is_ok());
    assert!(register.admits(&Scope::Venue("anything".into())).is_ok());
    assert!(register.admits(&Scope::Strategy("anything".into())).is_ok());
    assert!(register.admits(&Scope::Instrument("anything".into())).is_ok());
}

#[test]
fn desks_allocated_more_than_the_portfolio_are_refused() {
    let err = Register::new(
        amount(1_000_000),
        [("macro".to_string(), amount(700_000)), ("credit".to_string(), amount(400_000))],
    )
    .unwrap_err();

    assert_eq!(err, RegisterError::OverAllocated { desks: 1_100_000, portfolio: 1_000_000 });
    assert!(err.to_string().contains("1100000"), "both totals are stated: {err}");
}

#[test]
fn allocating_exactly_the_portfolio_is_permitted() {
    let register = Register::new(
        amount(1_000_000),
        [("macro".to_string(), amount(600_000)), ("credit".to_string(), amount(400_000))],
    )
    .unwrap();

    assert_eq!(register.unallocated(), amount(0));
}

#[test]
fn under_allocation_passes_and_the_remainder_is_stated() {
    assert_eq!(two_desks().unallocated(), amount(100_000));
}

#[test]
fn a_single_book_register_is_valid_and_says_so() {
    let register = Register::single_book(amount(1_000_000));

    assert!(register.is_single_book());
    assert_eq!(register.unallocated(), amount(1_000_000));
    assert_eq!(register.names().count(), 0);
}

#[test]
fn an_entity_naming_no_desk_is_refused_when_desks_are_in_use() {
    let register = two_desks();

    assert_eq!(register.admits_entity(&entity("")).unwrap_err(), RegisterError::Unassigned);
    assert_eq!(register.admits_entity(&entity("   ")).unwrap_err(), RegisterError::Unassigned);
    assert_eq!(
        register.admits_entity(&entity("unknown")).unwrap_err(),
        RegisterError::UnknownDesk { name: "unknown".into() }
    );
    assert!(register.admits_entity(&entity("macro")).is_ok());
}

#[test]
fn a_single_book_admits_every_entity() {
    let register = Register::single_book(amount(1_000_000));

    assert!(register.admits_entity(&entity("")).is_ok());
    assert!(register.admits_entity(&entity("anything")).is_ok());
}

/// Source-level check: no position or pnl record may appear in this crate.
#[test]
fn this_component_holds_no_position_or_pnl_record() {
    let source = include_str!("../../../src/desks/mod.rs");

    for forbidden in ["struct DeskPosition", "struct DeskPnl", "quantity", "position:"] {
        assert!(
            !source.contains(forbidden),
            "the register carries '{forbidden}', which is a second answer to the same question"
        );
    }
}

#[test]
fn a_desk_that_holds_risk_cannot_be_removed() {
    let mut register = two_desks();

    assert_eq!(
        register.remove("macro", true).unwrap_err(),
        RegisterError::StillHoldsRisk { name: "macro".into() }
    );
    assert!(register.knows("macro"));

    assert!(register.remove("macro", false).is_ok());
    assert!(!register.knows("macro"));
}

#[test]
fn removing_a_desk_that_was_never_registered_is_refused() {
    let mut register = two_desks();
    assert!(matches!(
        register.remove("never-existed", false),
        Err(RegisterError::UnknownDesk { .. })
    ));
}

#[test]
fn a_desk_with_no_name_is_refused() {
    assert_eq!(
        Register::new(amount(100), [("  ".to_string(), amount(10))]).unwrap_err(),
        RegisterError::EmptyName
    );
}

#[test]
fn a_desk_allocated_nothing_is_refused_rather_than_read_as_a_policy() {
    assert_eq!(
        Register::new(amount(100), [("macro".to_string(), amount(0))]).unwrap_err(),
        RegisterError::ZeroAllocation { name: "macro".into() }
    );
}

#[test]
fn a_desk_registered_twice_is_refused() {
    assert_eq!(
        Register::new(
            amount(1_000),
            [("macro".to_string(), amount(100)), ("macro".to_string(), amount(200))]
        )
        .unwrap_err(),
        RegisterError::DuplicateDesk { name: "macro".into() }
    );
}
