// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What desk and strategy figures must never do: understate, or be produced from a part of the book.

use std::collections::BTreeMap;

use bijective_limits::{Amount, LimitKind, Scope, Threshold};
use bijective_model::{HeldPosition, InstrumentId, Owner, VenueSymbol};
use bijective_venue::profile::{
    Instrument, InstrumentKind, InstrumentStatus, PrecisionRules, PriceValidity,
};

use super::*;
use crate::valuation::currency::Conversion;
use crate::valuation::{Mark, Unvalued};

/// Quantity scale 3, price scale 2, marked at 100.00: one whole unit is worth 100.00.
fn instrument(symbol: &str) -> Instrument {
    Instrument {
        symbol: VenueSymbol::new(symbol),
        id: InstrumentId::new(symbol),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: None,
        precision: PrecisionRules {
            price_scale: 2,
            quantity_scale: 3,
            price_tick: 1,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 1,
        status: InstrumentStatus::Trading,
    }
}

fn catalogue() -> BTreeMap<InstrumentId, Instrument> {
    [instrument("aaa"), instrument("bbb")].into_iter().map(|i| (i.id.clone(), i)).collect()
}

fn marks() -> BTreeMap<InstrumentId, Mark> {
    ["aaa", "bbb"]
        .into_iter()
        .map(|s| (InstrumentId::new(s), Mark { price_scaled: 10_000, observed_at_ns: 50 }))
        .collect()
}

/// Whole units of 100.00 each, at the money scale.
fn worth(units_thousandths: i128) -> Threshold {
    Threshold::Money(Amount::from_scaled(units_thousandths * 10_000_000))
}

fn held(instrument: &str, quantity_scaled: i128) -> HeldPosition {
    HeldPosition {
        venue: "venue-a".into(),
        instrument: InstrumentId::new(instrument),
        quantity_scaled,
        entry_price_scaled: 10_000,
    }
}

fn by(desk: &str, strategy: Option<&str>) -> Option<Owner> {
    Some(Owner { desk: desk.into(), strategy: strategy.map(str::to_string) })
}

fn filled(owner: Option<Owner>, instrument: &str, filled_scaled: i128) -> Placed {
    Placed {
        owner,
        venue: "venue-a".into(),
        instrument: InstrumentId::new(instrument),
        filled_scaled,
        open_buy_scaled: 0,
        open_sell_scaled: 0,
    }
}

fn measure(held: &[HeldPosition], placed: &[Placed], named: &[Scope]) -> Attributed {
    let (marks, catalogue) = (marks(), catalogue());
    attributed(held, placed, &mut Conversion::new(Pricing::single(&marks, &catalogue), None), named)
        .expect("measured")
}

fn figure(measured: &Attributed, scope: &Scope, kind: LimitKind) -> Option<Threshold> {
    measured.figures.iter().find(|(s, k, _)| s == scope && *k == kind).map(|(_, _, v)| *v)
}

fn desk(name: &str) -> Scope {
    Scope::Desk(name.into())
}

fn strategy(name: &str) -> Scope {
    Scope::Strategy(name.into())
}

/// Two desks on one instrument: the venue nets them, the desks do not.
#[test]
fn each_desk_holds_what_its_own_fills_hold() {
    let measured = measure(
        &[held("aaa", 500)],
        &[
            filled(by("alpha", Some("trend")), "aaa", 2_000),
            filled(by("beta", Some("revert")), "aaa", -1_500),
        ],
        &[],
    );

    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::GrossExposure), Some(worth(2_000)));
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::NetExposure), Some(worth(2_000)));
    assert_eq!(figure(&measured, &desk("beta"), LimitKind::GrossExposure), Some(worth(1_500)));
    assert_eq!(figure(&measured, &desk("beta"), LimitKind::NetExposure), Some(worth(-1_500)));
    assert_eq!(figure(&measured, &strategy("trend"), LimitKind::GrossExposure), Some(worth(2_000)));
    assert_eq!(measured.observed_at_ns, 50, "a figure is as old as the mark it was priced at");
}

/// A working order may fill before the next read: the figure holds the worse of all of it
/// filling and none of it.
#[test]
fn a_working_order_counts_what_it_could_still_fill() {
    let mut working = filled(by("alpha", Some("trend")), "aaa", 1_000);
    working.open_buy_scaled = 3_000;
    let mut reducing = filled(by("alpha", Some("trend")), "bbb", -1_000);
    reducing.open_buy_scaled = 1_000;

    let measured = measure(&[held("aaa", 1_000), held("bbb", -1_000)], &[working, reducing], &[]);

    // aaa: 1 held, 3 more may buy: 4. bbb: -1 held, a buy of 1 can only flatten it: 1.
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::GrossExposure), Some(worth(5_000)));
    // Net counts every buy filling: aaa 4, bbb 0.
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::NetExposure), Some(worth(4_000)));
}

/// What the venue holds beyond the recorded fills is nobody's that the record can show, so it
/// is charged to every desk and every strategy, never left out.
#[test]
fn a_position_the_fills_do_not_explain_counts_against_everyone() {
    let measured = measure(
        &[held("aaa", 3_000), held("bbb", -2_000)],
        &[filled(by("alpha", Some("trend")), "aaa", 1_000)],
        &[desk("beta")],
    );

    // alpha: its own 1 of aaa, 2 of aaa unexplained, 2 of bbb unexplained.
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::GrossExposure), Some(worth(5_000)));
    // Net is the end furthest from flat: all unexplained longs alpha's (3), or all shorts (-1).
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::NetExposure), Some(worth(3_000)));
    // beta placed nothing and is still charged everything unexplained.
    assert_eq!(figure(&measured, &desk("beta"), LimitKind::GrossExposure), Some(worth(4_000)));
    assert_eq!(figure(&measured, &strategy("trend"), LimitKind::GrossExposure), Some(worth(5_000)));
}

/// An order no strategy is named on belongs to no strategy figure, so its fills and what it
/// could still fill are charged to every strategy.
#[test]
fn an_order_without_a_strategy_counts_against_every_strategy() {
    let mut unnamed = filled(by("alpha", None), "aaa", 1_000);
    unnamed.open_sell_scaled = 2_000;

    let measured = measure(
        &[held("aaa", 2_000)],
        &[filled(by("alpha", Some("trend")), "aaa", 1_000), unnamed],
        &[strategy("revert")],
    );

    // trend: its 1, the unnamed 1 already filled, and the 2 it could still sell.
    assert_eq!(figure(&measured, &strategy("trend"), LimitKind::GrossExposure), Some(worth(4_000)));
    assert_eq!(
        figure(&measured, &strategy("revert"), LimitKind::GrossExposure),
        Some(worth(3_000))
    );
    // The desk owns both orders: 2 held, and selling 2 more can take it flat at worst.
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::GrossExposure), Some(worth(2_000)));
}

/// A declared limit needs a figure even for a desk that has placed nothing.
#[test]
fn a_named_desk_that_placed_nothing_has_a_figure_of_nothing() {
    let measured = measure(&[], &[], &[desk("alpha"), strategy("trend")]);

    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::GrossExposure), Some(worth(0)));
    assert_eq!(figure(&measured, &strategy("trend"), LimitKind::NetExposure), Some(worth(0)));
}

/// One order that cannot be priced leaves every desk and strategy unmeasured, not partly measured.
#[test]
fn an_order_that_cannot_be_priced_measures_nothing() {
    let marks = BTreeMap::new();
    let catalogue = catalogue();
    let mut working = filled(by("alpha", Some("trend")), "aaa", 0);
    working.open_buy_scaled = 1_000;

    let measured = attributed(
        &[],
        &[working],
        &mut Conversion::new(Pricing::single(&marks, &catalogue), None),
        &[desk("alpha")],
    );

    assert_eq!(measured, Err(Unvalued::NoMark { instrument: InstrumentId::new("aaa") }));
}

/// Desk figures summed across settlement currencies would add unlike money.
#[test]
fn a_desk_holding_two_settlement_currencies_is_not_measured() {
    let mut other = instrument("bbb");
    other.settlement_currency = "usdc".into();
    let catalogue: BTreeMap<InstrumentId, Instrument> =
        [instrument("aaa"), other].into_iter().map(|i| (i.id.clone(), i)).collect();
    let marks = marks();

    let measured = attributed(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &[filled(by("alpha", None), "aaa", 1_000), filled(by("alpha", None), "bbb", 1_000)],
        &mut Conversion::new(Pricing::single(&marks, &catalogue), None),
        &[],
    );

    assert!(matches!(measured, Err(Unvalued::ForeignSettlement { .. })), "{measured:?}");
}

/// A net limit is judged by magnitude, so a short the record does not explain counts as short.
#[test]
fn an_unexplained_short_counts_toward_the_net_furthest_from_flat() {
    let measured =
        measure(&[held("aaa", -3_000)], &[filled(by("alpha", Some("trend")), "aaa", -1_000)], &[]);

    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::NetExposure), Some(worth(-3_000)));
    assert_eq!(figure(&measured, &desk("alpha"), LimitKind::GrossExposure), Some(worth(3_000)));
}
