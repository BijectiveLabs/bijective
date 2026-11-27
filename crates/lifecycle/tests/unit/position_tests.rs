// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Positions from fills: exact first-in first-out accounting, reversal, and refusal.

use bijective_model::{Side, VenueSymbol};

use crate::position::{Lot, Position, PositionCause, PositionError};

fn cause(id: &str) -> PositionCause {
    PositionCause::Fill { fill_id: id.into() }
}

fn flat() -> Position {
    Position::flat(VenueSymbol::new("AAA"))
}

fn trade(p: &Position, id: &str, side: Side, price: i128, quantity: i128) -> Position {
    p.apply(cause(id), side, price, quantity).expect("applied").0
}

#[test]
fn buys_open_lots_in_order_at_their_own_prices() {
    let p = trade(&trade(&flat(), "a", Side::Buy, 100, 2), "b", Side::Buy, 110, 3);
    assert_eq!(p.quantity_scaled(), 5);
    let lots: Vec<Lot> = p.lots().copied().collect();
    assert_eq!(
        lots,
        vec![
            Lot { price_scaled: 100, quantity_scaled: 2 },
            Lot { price_scaled: 110, quantity_scaled: 3 }
        ]
    );
    assert_eq!(p.open_cost_scaled(), Ok(530));
}

/// Closing 3 of (2 at 100, 3 at 110) at 120 realizes 2*20 + 1*10, exactly.
#[test]
fn a_reduction_consumes_the_oldest_lots_and_realizes_exactly() {
    let long = trade(&trade(&flat(), "a", Side::Buy, 100, 2), "b", Side::Buy, 110, 3);
    let (p, change) = long.apply(cause("c"), Side::Sell, 120, 3).unwrap();
    assert_eq!(change.realized_scaled, 50);
    assert_eq!((change.quantity_before_scaled, change.quantity_after_scaled), (5, 2));
    assert_eq!(
        p.lots().copied().collect::<Vec<_>>(),
        vec![Lot { price_scaled: 110, quantity_scaled: 2 }]
    );
    assert_eq!(p.realized_scaled, 50);
}

#[test]
fn a_short_gains_when_it_closes_lower() {
    let short = trade(&flat(), "a", Side::Sell, 100, 4);
    assert_eq!(short.quantity_scaled(), -4);
    let (_, change) = short.apply(cause("b"), Side::Buy, 90, 4).unwrap();
    assert_eq!(change.realized_scaled, 40);
}

/// A fill larger than the position closes it and opens the remainder the other way, at the fill.
#[test]
fn a_fill_through_flat_reverses_the_position() {
    let long = trade(&flat(), "a", Side::Buy, 100, 2);
    let (p, change) = long.apply(cause("b"), Side::Sell, 105, 5).unwrap();
    assert_eq!(change.realized_scaled, 10);
    assert_eq!(p.quantity_scaled(), -3);
    assert_eq!(
        p.lots().copied().collect::<Vec<_>>(),
        vec![Lot { price_scaled: 105, quantity_scaled: 3 }]
    );
}

#[test]
fn an_identity_is_applied_once_and_a_refusal_changes_nothing() {
    let p = trade(&flat(), "a", Side::Buy, 100, 1);
    assert!(matches!(
        p.apply(cause("a"), Side::Buy, 100, 1),
        Err(PositionError::AlreadyApplied { .. })
    ));
    assert!(matches!(
        p.apply(cause("z"), Side::Buy, 0, 1),
        Err(PositionError::NotPositive { field: "price", .. })
    ));
    assert!(matches!(
        p.apply(cause("z"), Side::Buy, 1, -1),
        Err(PositionError::NotPositive { .. })
    ));
    assert_eq!(p.quantity_scaled(), 1);
}

#[test]
fn a_figure_beyond_128_bits_is_refused_rather_than_wrapped() {
    let p = trade(&flat(), "a", Side::Buy, i128::MAX / 2, 4);
    assert!(matches!(p.apply(cause("b"), Side::Sell, 1, 4), Err(PositionError::Overflow { .. })));
    assert_eq!(p.quantity_scaled(), 4);
    let huge = trade(&flat(), "c", Side::Buy, i128::MAX, 2);
    assert!(matches!(huge.open_cost_scaled(), Err(PositionError::Overflow { .. })));
}

#[test]
fn an_adjustment_is_its_own_audited_cause() {
    let adjust =
        PositionCause::Adjustment { adjustment_id: "adj-1".into(), reason: "transfer in".into() };
    let (p, change) = flat().apply(adjust.clone(), Side::Buy, 100, 7).unwrap();
    assert_eq!(change.cause, adjust);
    assert!(p.has_applied("adj-1"));
}

#[test]
fn fees_are_kept_per_asset_and_scale() {
    let p = flat()
        .with_fee("usdt", 6, 1_500)
        .unwrap()
        .with_fee("usdt", 6, 500)
        .unwrap()
        .with_fee("bnb", 8, 3)
        .unwrap();
    assert_eq!(p.fees.get(&("usdt".to_string(), 6)), Some(&2_000));
    assert_eq!(p.fees.len(), 2);
}
