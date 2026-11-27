// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use std::collections::BTreeSet;

use bijective_limits::Amount;
use bijective_model::{InstrumentId, Proposal, Side};

use super::admission::{Mandate, MandateBook, Refusal};
use super::collar::*;
use crate::valuation::Mark;

fn mandates(collar_bps: u32) -> MandateBook {
    let mut book = MandateBook::new();
    book.declare(
        "alpha",
        Mandate {
            instruments: BTreeSet::from([InstrumentId::new("btc-perp")]),
            venue_preference: vec!["venue-a".into()],
            maximum_per_order: Amount::from_scaled(1_000_000_000),
            price_collar_bps: collar_bps,
        },
    );
    book
}

fn proposal(side: Side, price_scaled: Option<i128>) -> Proposal {
    Proposal {
        desk: "alpha".into(),
        instrument: InstrumentId::new("btc-perp"),
        side,
        quantity_scaled: 1,
        price_scaled,
        cap_scaled: Some(20_000),
        client_id: "c-1".into(),
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

fn mark_at(price_scaled: i128, observed_at_ns: i128) -> CollarMark {
    CollarMark {
        mark: Some(Mark { price_scaled, observed_at_ns }),
        as_of_ns: 1_000,
        max_age_ns: 500,
    }
}

#[test]
fn a_buy_at_the_edge_of_the_collar_is_admitted_and_one_tick_beyond_is_refused() {
    let at = mark_at(10_000, 900);
    assert_eq!(within_collar(&mandates(500), &proposal(Side::Buy, Some(10_500)), &at), Ok(()));
    assert_eq!(
        within_collar(&mandates(500), &proposal(Side::Buy, Some(10_501)), &at),
        Err(Refusal::OutsidePriceCollar { price: 10_501, mark: 10_000, collar_bps: 500 })
    );
}

#[test]
fn a_sell_below_the_collar_is_refused() {
    let at = mark_at(10_000, 900);
    assert_eq!(within_collar(&mandates(500), &proposal(Side::Sell, Some(9_500)), &at), Ok(()));
    assert!(matches!(
        within_collar(&mandates(500), &proposal(Side::Sell, Some(9_499)), &at),
        Err(Refusal::OutsidePriceCollar { .. })
    ));
}

/// A price that does not cross rests without harm, however far from the mark.
#[test]
fn a_passive_price_far_from_the_mark_is_admitted() {
    let at = mark_at(10_000, 900);
    assert_eq!(within_collar(&mandates(500), &proposal(Side::Buy, Some(1)), &at), Ok(()));
    assert_eq!(within_collar(&mandates(500), &proposal(Side::Sell, Some(90_000)), &at), Ok(()));
}

#[test]
fn a_limit_price_without_a_mark_is_refused() {
    let at = CollarMark { mark: None, as_of_ns: 1_000, max_age_ns: 500 };
    assert_eq!(
        within_collar(&mandates(500), &proposal(Side::Buy, Some(10_000)), &at),
        Err(Refusal::NoMarkForCollar { instrument: InstrumentId::new("btc-perp") })
    );
}

#[test]
fn a_mark_older_than_the_bound_is_refused() {
    let at = mark_at(10_000, 400);
    assert_eq!(
        within_collar(&mandates(500), &proposal(Side::Buy, Some(10_000)), &at),
        Err(Refusal::MarkTooOldForCollar { age_ns: 600, max_age_ns: 500 })
    );
}

/// A market order's cap never reaches the venue. The per-order maximum bounds it instead.
#[test]
fn a_market_order_is_not_collared() {
    let at = CollarMark { mark: None, as_of_ns: 1_000, max_age_ns: 500 };
    assert_eq!(within_collar(&mandates(500), &proposal(Side::Buy, None), &at), Ok(()));
}

#[test]
fn a_wider_collar_declared_by_the_desk_admits_more() {
    let at = mark_at(10_000, 900);
    assert_eq!(within_collar(&mandates(2_000), &proposal(Side::Buy, Some(12_000)), &at), Ok(()));
}

fn stop(side: Side, stop_price: i128, price_scaled: Option<i128>) -> Proposal {
    let mut p = proposal(side, price_scaled);
    p.terms.stop_price_scaled = Some(stop_price);
    p
}

/// A buy stop sits above the mark and a sell stop below it. One the market has passed is refused.
#[test]
fn a_stop_already_crossed_is_refused() {
    let at = mark_at(10_000, 900);
    assert_eq!(within_collar(&mandates(500), &stop(Side::Buy, 10_100, None), &at), Ok(()));
    assert_eq!(
        within_collar(&mandates(500), &stop(Side::Buy, 10_000, None), &at),
        Err(Refusal::StopAlreadyCrossed { stop: 10_000, mark: 10_000 })
    );
    assert_eq!(within_collar(&mandates(500), &stop(Side::Sell, 9_900, None), &at), Ok(()));
    assert!(matches!(
        within_collar(&mandates(500), &stop(Side::Sell, 10_050, None), &at),
        Err(Refusal::StopAlreadyCrossed { .. })
    ));
}

/// A stop-limit becomes a limit order at its stop, so its price is collared around the stop.
#[test]
fn a_stop_limit_is_collared_around_its_stop() {
    let at = mark_at(10_000, 900);
    assert_eq!(within_collar(&mandates(500), &stop(Side::Buy, 12_000, Some(12_600)), &at), Ok(()));
    assert_eq!(
        within_collar(&mandates(500), &stop(Side::Buy, 12_000, Some(12_601)), &at),
        Err(Refusal::OutsidePriceCollar { price: 12_601, mark: 12_000, collar_bps: 500 })
    );
}

#[test]
fn a_stop_market_without_a_mark_is_refused() {
    let at = CollarMark { mark: None, as_of_ns: 1_000, max_age_ns: 500 };
    assert!(matches!(
        within_collar(&mandates(500), &stop(Side::Sell, 9_000, None), &at),
        Err(Refusal::NoMarkForCollar { .. })
    ));
}
