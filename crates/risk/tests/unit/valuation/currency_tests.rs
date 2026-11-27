// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a conversion into the reporting currency must refuse, and what it must never understate.

use bijective_limits::{ExposureSnapshot, LimitKind, Scope, Threshold};
use bijective_model::{HeldPosition, VenueSymbol};
use bijective_venue::profile::{InstrumentStatus, PrecisionRules, PriceValidity};

use super::*;
use crate::valuation::{Mark, Venues, value};

fn listed(
    id: &str,
    kind: InstrumentKind,
    settles: &str,
    base: &str,
    scales: (u8, u8),
) -> Instrument {
    Instrument {
        symbol: VenueSymbol::new(id),
        id: InstrumentId::new(id),
        kind,
        settlement_currency: settles.into(),
        quote_currency: settles.into(),
        base_currency: Some(base.into()),
        precision: PrecisionRules {
            price_scale: scales.0,
            quantity_scale: scales.1,
            price_tick: 1,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 1,
        status: InstrumentStatus::Trading,
    }
}

/// An equity in USD, a perpetual in USDT, and the USDT/USD market, written as a venue writes it.
fn catalogue() -> BTreeMap<InstrumentId, Instrument> {
    [
        listed("eq", InstrumentKind::Equity, "usd", "eq", (2, 0)),
        listed("perp", InstrumentKind::Perpetual, "usdt", "eth", (2, 3)),
        listed("usdtusd", InstrumentKind::Spot, "USD", "USDT", (4, 2)),
    ]
    .into_iter()
    .map(|i| (i.id.clone(), i))
    .collect()
}

/// Each rate may be at most 60 ns old.
fn sources(pairs: &[(&str, &str)]) -> BTreeMap<String, RateSource> {
    pairs
        .iter()
        .map(|(c, i)| {
            ((*c).to_string(), RateSource { instrument: InstrumentId::new(*i), max_age_ns: 60 })
        })
        .collect()
}

fn usd(catalogue: &BTreeMap<InstrumentId, Instrument>) -> Rates {
    Rates::declared("USD", &sources(&[("USDT", "usdtusd")]), catalogue).expect("declared")
}

/// The equity at 50.00, the perpetual at 100.00, USDT at 0.9990 USD observed at `rate_at`.
fn marks(rate: i128, rate_at: i128) -> BTreeMap<InstrumentId, Mark> {
    [("eq", 5_000, 100), ("perp", 10_000, 100), ("usdtusd", rate, rate_at)]
        .into_iter()
        .map(|(id, price_scaled, observed_at_ns)| {
            (InstrumentId::new(id), Mark { price_scaled, observed_at_ns })
        })
        .collect()
}

fn held(id: &str, quantity_scaled: i128, entry_price_scaled: i128) -> HeldPosition {
    HeldPosition {
        venue: "venue-a".into(),
        instrument: InstrumentId::new(id),
        quantity_scaled,
        entry_price_scaled,
    }
}

fn gross(snapshot: &ExposureSnapshot) -> Option<Threshold> {
    snapshot
        .figures
        .iter()
        .find(|(s, k, _)| *s == Scope::Portfolio && *k == LimitKind::GrossExposure)
        .map(|(_, _, v)| *v)
}

fn money(scaled: i128) -> Threshold {
    Threshold::Money(Amount::from_scaled(scaled))
}

#[test]
fn a_book_in_two_currencies_is_valued_in_the_reporting_currency() {
    let catalogue = catalogue();
    let rates = usd(&catalogue);
    let marks = marks(9_990, 100);
    let pricing = Pricing { marks: &marks, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };

    let v = value(
        &[held("eq", 2, 5_000), held("perp", 1_000, 9_000)],
        &pricing,
        &Venues::default(),
        100,
    );

    assert!(v.unvalued.is_empty(), "{:?}", v.unvalued);
    // 100.00 USD of shares and 100.00 USDT at 0.9990: 199.90 USD.
    assert_eq!(gross(&v.snapshot), Some(money(19_990_000_000)));
    // The perpetual gained 10.00 USDT, which is 9.99 USD.
    assert_eq!(v.unrealized, Some(Amount::from_scaled(999_000_000)));
    assert_eq!(v.currency.as_deref(), Some("usd"));
}

#[test]
fn the_reporting_currency_holds_when_the_first_position_is_foreign() {
    let catalogue = catalogue();
    let rates = usd(&catalogue);
    let marks = marks(9_990, 100);
    let pricing = Pricing { marks: &marks, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };

    let v = value(&[held("perp", 1_000, 10_000)], &pricing, &Venues::default(), 100);

    assert_eq!(gross(&v.snapshot), Some(money(9_990_000_000)));
    assert_eq!(v.currency.as_deref(), Some("usd"));
}

#[test]
fn a_rate_market_without_a_positive_mark_leaves_the_book_unvalued() {
    let catalogue = catalogue();
    let rates = usd(&catalogue);
    for marks in [marks(0, 100), {
        let mut m = marks(9_990, 100);
        m.remove(&InstrumentId::new("usdtusd"));
        m
    }] {
        let pricing =
            Pricing { marks: &marks, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };

        let v = value(&[held("perp", 1_000, 10_000)], &pricing, &Venues::default(), 100);

        assert!(v.snapshot.figures.is_empty(), "{:?}", v.snapshot.figures);
        assert_eq!(v.unrealized, None);
        assert!(
            matches!(&v.unvalued[..], [Unvalued::NoRate { currency, .. }] if currency == "usdt"),
            "{:?}",
            v.unvalued
        );
    }
}

#[test]
fn a_rate_older_than_its_bound_leaves_the_book_unvalued_and_a_younger_one_does_not_age_it() {
    let catalogue = catalogue();
    let rates = usd(&catalogue);
    let old = marks(9_990, 39);
    let pricing = Pricing { marks: &old, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };

    let v = value(&[held("perp", 1_000, 10_000)], &pricing, &Venues::default(), 100);

    assert!(v.snapshot.figures.is_empty());
    assert!(
        matches!(&v.unvalued[..], [Unvalued::NoRate { age_ns: Some(61), .. }]),
        "{:?}",
        v.unvalued
    );

    let within = marks(9_990, 40);
    let pricing = Pricing { marks: &within, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };
    let v = value(&[held("perp", 1_000, 10_000)], &pricing, &Venues::default(), 100);
    assert!(v.unvalued.is_empty(), "{:?}", v.unvalued);
    assert_eq!(v.snapshot.observed_at_ns, 100, "the rate's age is its own bound's to judge");
}

#[test]
fn a_currency_with_no_declared_rate_is_unvalued() {
    let mut catalogue = catalogue();
    catalogue.insert(
        InstrumentId::new("hl"),
        listed("hl", InstrumentKind::Perpetual, "usdc", "btc", (2, 3)),
    );
    let rates = usd(&catalogue);
    let mut marks = marks(9_990, 100);
    marks.insert(InstrumentId::new("hl"), Mark { price_scaled: 10_000, observed_at_ns: 100 });
    let pricing = Pricing { marks: &marks, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };

    let v = value(
        &[held("eq", 2, 5_000), held("hl", 1_000, 10_000)],
        &pricing,
        &Venues::default(),
        100,
    );

    assert!(v.snapshot.figures.is_empty());
    assert!(
        v.unvalued.iter().any(|u| matches!(
            u,
            Unvalued::ForeignSettlement { settles_in, book_settles_in, .. }
                if settles_in == "usdc" && book_settles_in == "usd"
        )),
        "{:?}",
        v.unvalued
    );
}

#[test]
fn a_conversion_rounds_away_from_zero() {
    let catalogue = catalogue();
    let rates = usd(&catalogue);
    let marks = marks(9_999, 100);
    let pricing = Pricing { marks: &marks, catalogue: &catalogue, rates: &rates, as_of_ns: 100 };
    let mut book = Conversion::new(pricing, None);
    let on = InstrumentId::new("perp");

    // 0.00000001 USDT at 0.9999 is 0.0000000099990 USD: one unit up, never zero.
    assert_eq!(book.into_book(Amount::from_scaled(1), "usdt", &on), Ok(Amount::from_scaled(1)));
    assert_eq!(book.into_book(Amount::from_scaled(-1), "usdt", &on), Ok(Amount::from_scaled(-1)));
}

#[test]
fn without_a_reporting_currency_the_first_amount_fixes_the_book() {
    let catalogue = catalogue();
    let marks = marks(9_990, 100);
    let mut book = Conversion::new(Pricing::single(&marks, &catalogue), None);
    let on = InstrumentId::new("perp");

    assert!(book.into_book(Amount::from_scaled(5), "USDT", &on).is_ok());
    assert!(matches!(
        book.into_book(Amount::from_scaled(5), "usd", &on),
        Err(Unvalued::ForeignSettlement { .. })
    ));
}

#[test]
fn a_rate_market_that_cannot_carry_the_rate_is_refused() {
    let catalogue = catalogue();
    let declared = |pairs: &[(&str, &str)]| Rates::declared("usd", &sources(pairs), &catalogue);

    assert!(matches!(declared(&[("usd", "usdtusd")]), Err(RateRefused::OwnCurrency { .. })));
    assert!(matches!(declared(&[("usdt", "absent")]), Err(RateRefused::NotCatalogued { .. })));
    assert!(matches!(declared(&[("usdt", "perp")]), Err(RateRefused::NotSpot { .. })));
    assert!(matches!(
        Rates::declared("eur", &sources(&[("usdt", "usdtusd")]), &catalogue),
        Err(RateRefused::WrongPair { .. })
    ));
    assert!(matches!(declared(&[("usdc", "usdtusd")]), Err(RateRefused::WrongPair { .. })));
    assert!(declared(&[("USDT", "usdtusd")]).is_ok());
    let mut unbounded = sources(&[("usdt", "usdtusd")]);
    unbounded.values_mut().for_each(|s| s.max_age_ns = 0);
    assert!(matches!(
        Rates::declared("usd", &unbounded, &catalogue),
        Err(RateRefused::NoAgeBound { .. })
    ));
}
