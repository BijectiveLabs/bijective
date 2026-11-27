// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! These types carry no behaviour, so what is worth testing is what they refuse to lose.

use super::*;

/// A short position is a negative quantity, not a side flag beside a magnitude.
#[test]
fn a_short_position_is_signed_rather_than_flagged() {
    let short = PositionSnapshot {
        instrument: "BTCUSDT".into(),
        quantity_scaled: -250,
        entry_price_scaled: 7_600_000,
    };
    assert!(short.quantity_scaled < 0);
}

/// A market order carries no price. `None` is the statement. A zero would be a price.
#[test]
fn a_market_order_has_no_price_rather_than_a_zero_one() {
    let market = OrderRequest {
        client_id: "bl-1".into(),
        instrument: "BTCUSDT".into(),
        side: Side::Buy,
        quantity_scaled: 1_000,
        price_scaled: None,
        reduce_only: false,
        terms: Default::default(),
    };
    assert_eq!(market.price_scaled, None);
}

/// An account reading with no time on it cannot be reconciled *as of* anything.
#[test]
fn an_account_snapshot_carries_when_it_was_taken() {
    let snapshot = AccountSnapshot {
        observed_at_ns: 1_700_000_000_000_000_000,
        positions: vec![],
        margin: None,
    };
    assert!(snapshot.observed_at_ns > 0);
}

/// Scaled integers throughout. A float anywhere would round at the first hop.
#[test]
fn every_quantity_is_an_integer_wide_enough_for_the_platforms_scale() {
    let top = BookTop {
        instrument: "BTCUSDT".into(),
        sequence: Sequence::Numbered(u64::MAX),
        venue_ts_ms: 0,
        bid_price: i128::MAX,
        bid_quantity: i128::MIN,
        ask_price: 0,
        ask_quantity: 0,
        bid_depth: u32::MAX,
        ask_depth: 0,
    };
    assert_eq!(top.bid_price, i128::MAX);
    assert_eq!(top.bid_quantity, i128::MIN);
}

/// All conditions must hold, not any of them.
#[test]
fn a_clearance_needs_every_condition() {
    assert!(Clearance::granted(0, 0, true, 1).is_some());

    assert!(Clearance::granted(1, 0, true, 1).is_none(), "a breach");
    assert!(Clearance::granted(0, 1, true, 1).is_none(), "something unjudged");
    assert!(Clearance::granted(0, 0, false, 1).is_none(), "state too old to stand behind");
    assert!(Clearance::granted(2, 3, false, 1).is_none(), "all three wrong");
}

#[test]
fn a_clearance_records_when_it_was_granted() {
    let c = Clearance::granted(0, 0, true, 1_700_000_000_000_000_000).unwrap();
    assert_eq!(c.granted_at_ns(), 1_700_000_000_000_000_000);
}

/// The seal means a caller cannot write the struct literal: a clearance is granted by the figures
/// or minted for an operator's flatten order, and by nothing else (decision 0010).
#[test]
fn the_only_ways_to_hold_one_are_the_two_constructors() {
    let source = include_str!("../../src/lib.rs");
    assert!(source.contains("seal: ()"), "the private field is what closes the literal path");
    assert_eq!(source.matches("Self { granted_at_ns").count(), 2, "two places construct one");
    let reduction = source.split("pub fn for_operator_reduction").nth(1).expect("declared");
    assert!(
        reduction.split("pub fn").next().is_some_and(|body| body.contains("Self { granted_at_ns"))
    );
}
