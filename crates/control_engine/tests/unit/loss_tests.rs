// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The daily loss figure: what it is made of, and when it refuses to exist.

use std::collections::BTreeMap;

use bijective_limits::{Amount, LimitKind, Scope, Threshold};
use bijective_model::{InstrumentId, VenueSymbol};
use bijective_venue::profile::{
    Instrument, InstrumentKind, InstrumentStatus, PrecisionRules, PriceValidity,
};

use bijective_risk::valuation::currency::{Conversion, RateSource, Rates};
use bijective_risk::valuation::{Mark, Pricing};

use super::{Today, Unmeasured, daily_loss_figure};
use crate::orders::{Charged, Realized};

fn instrument() -> Instrument {
    Instrument {
        symbol: VenueSymbol::new("aaa"),
        id: InstrumentId::new("aaa"),
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

fn instruments() -> BTreeMap<(String, String), Instrument> {
    BTreeMap::from([(("venue-a".to_string(), "aaa".to_string()), instrument())])
}

static NO_MARKS: BTreeMap<InstrumentId, Mark> = BTreeMap::new();
static NO_CATALOGUE: BTreeMap<InstrumentId, Instrument> = BTreeMap::new();

/// A book with no reporting currency: the first amount fixes it.
fn single() -> Conversion<'static> {
    Conversion::new(Pricing::single(&NO_MARKS, &NO_CATALOGUE), None)
}

fn realized(venue: &str, instrument: &str, realized_scaled: i128) -> Realized {
    Realized {
        venue: venue.into(),
        instrument: VenueSymbol::new(instrument),
        realized_scaled,
        at_ns: 1,
    }
}

fn money(scaled: i128) -> Threshold {
    Threshold::Money(Amount::from_scaled(scaled))
}

#[test]
fn the_loss_is_realised_today_plus_the_unrealised_movement_turned_over() {
    // Realised -1000 (at scale 5) is -0.01000, i.e. -1_000_000 at the money scale.
    let fills = [realized("venue-a", "aaa", -1_000)];
    let figure = daily_loss_figure(
        Some(Amount::from_scaled(-500_000)),
        Amount::from_scaled(-2_000_000),
        Today { realized: &fills, charged: &[], account: &[] },
        &instruments(),
        &mut single(),
    )
    .expect("measured")
    .figure;
    // Result: -1_000_000 + (-2_000_000 - -500_000) = -2_500_000. The loss is its opposite.
    assert_eq!(figure, (Scope::Portfolio, LimitKind::DailyLoss, money(2_500_000)));
}

/// The figure reaches the existing judge: a loss past the maximum is a breach of `DailyLoss`, a
/// gain is not.
#[test]
fn a_loss_past_the_maximum_breaches_and_a_gain_does_not() {
    use bijective_limits::{ExposureSnapshot, Limit, evaluate};

    let limit = Limit::declare(Scope::Portfolio, LimitKind::DailyLoss, money(1_000_000))
        .expect("a declarable limit");
    let judged = |loss_scaled: i128| {
        // baseline 0, realised 0, so the loss is the opposite of the unrealised movement.
        let figure = daily_loss_figure(
            Some(Amount::from_scaled(0)),
            Amount::from_scaled(-loss_scaled),
            Today { realized: &[], charged: &[], account: &[] },
            &instruments(),
            &mut single(),
        )
        .expect("measured")
        .figure;
        let snapshot = ExposureSnapshot { observed_at_ns: 10, figures: vec![figure] };
        evaluate(std::slice::from_ref(&limit), &snapshot, 10, 100)
    };
    assert_eq!(judged(1_000_001).breaches.len(), 1);
    assert!(judged(1_000_000).breaches.is_empty(), "at the maximum is still permitted");
    assert!(judged(-5).breaches.is_empty());
    assert!(judged(0).unjudged.is_empty());
}

#[test]
fn without_a_baseline_or_a_valued_book_there_is_no_figure() {
    let none = daily_loss_figure(
        None,
        Amount::from_scaled(0),
        Today { realized: &[], charged: &[], account: &[] },
        &instruments(),
        &mut single(),
    );
    assert_eq!(none, Err(Unmeasured::NoBaseline));
}

/// A realised result that cannot be priced is not zero: a loss limit judged on part of the day
/// would let the part it could not read pass.
#[test]
fn a_fill_on_an_instrument_the_catalogue_does_not_carry_leaves_the_loss_unmeasured() {
    let fills = [realized("venue-a", "zzz", -5)];
    let unknown = daily_loss_figure(
        Some(Amount::from_scaled(0)),
        Amount::from_scaled(0),
        Today { realized: &fills, charged: &[], account: &[] },
        &instruments(),
        &mut single(),
    );
    assert!(matches!(unknown, Err(Unmeasured::UnknownInstrument { .. })), "{unknown:?}");
}

fn charged(asset: &str, amount_scaled: i128) -> Charged {
    Charged {
        venue: "venue-a".into(),
        instrument: VenueSymbol::new("aaa"),
        fee: Some(bijective_lifecycle::Fee { asset: asset.into(), amount_scaled, scale: 4 }),
        at_ns: 1,
    }
}

/// A paid fee in the book's currency adds to the loss whatever case the venue writes it in.
/// Anything that cannot be valued is counted and left out, and the count is reported.
#[test]
fn a_paid_fee_in_the_quote_currency_adds_to_the_loss_and_others_are_counted() {
    let mut unstated = charged("usdt", 1);
    unstated.fee = None;
    // 0.5000 usdt and 0.5000 USDT paid are 100_000_000 at the money scale.
    let fees = [
        charged("usdt", 5_000),
        charged("USDT", 5_000),
        charged("", 7),
        charged("bnb", 9),
        charged("usdt", -3),
        unstated,
    ];
    let loss = daily_loss_figure(
        Some(Amount::from_scaled(0)),
        Amount::from_scaled(0),
        Today { realized: &[], charged: &fees, account: &[] },
        &instruments(),
        &mut single(),
    )
    .expect("measured");
    assert_eq!(loss.figure, (Scope::Portfolio, LimitKind::DailyLoss, money(100_000_000)));
    assert_eq!(loss.fees_not_valued, 4, "empty asset, another asset, a rebate, none stated");
}

fn drawn(result: i128, peak: Option<i128>) -> Threshold {
    super::drawdown_figure(Amount::from_scaled(result), peak.map(Amount::from_scaled))
        .expect("measured")
        .2
}

/// The day opens at a result of zero, so a day that only lost has drawn down by all of it.
#[test]
fn the_drawdown_is_measured_from_the_days_highest_result() {
    assert_eq!(drawn(-700, None), money(700), "from the open");
    assert_eq!(drawn(400, Some(1_000)), money(600), "from the recorded high");
    assert_eq!(drawn(1_200, Some(1_000)), money(0), "at a new high nothing is drawn down");
    assert_eq!(drawn(-50, Some(-100)), money(50), "a high below the open is not the high");
}

#[test]
fn the_days_result_is_the_loss_turned_over() {
    let loss = daily_loss_figure(
        Some(Amount::from_scaled(0)),
        Amount::from_scaled(-300),
        Today { realized: &[], charged: &[], account: &[] },
        &instruments(),
        &mut single(),
    )
    .expect("measured");
    assert_eq!(super::day_result(&loss), Some(Amount::from_scaled(-300)));
}

/// A realised result and a fee in the instrument's currency are converted at the declared rate,
/// and a currency with no declared rate leaves the loss unmeasured.
#[test]
fn a_loss_in_another_currency_is_converted_into_the_reporting_currency() {
    let mut rate = instrument();
    rate.id = InstrumentId::new("usdtusd");
    rate.kind = InstrumentKind::Spot;
    rate.base_currency = Some("usdt".into());
    rate.quote_currency = "usd".into();
    let catalogue = BTreeMap::from([(rate.id.clone(), rate)]);
    let marks = BTreeMap::from([(
        InstrumentId::new("usdtusd"),
        Mark { price_scaled: 50, observed_at_ns: 9 },
    )]);
    let source = RateSource { instrument: InstrumentId::new("usdtusd"), max_age_ns: 10 };
    let sources = BTreeMap::from([("usdt".to_string(), source)]);
    let rates = Rates::declared("usd", &sources, &catalogue).expect("declared");
    let pricing = Pricing { marks: &marks, catalogue: &catalogue, rates: &rates, as_of_ns: 9 };
    let mut book = Conversion::new(pricing, Some("usd"));
    // -0.01000 USDT at 0.50 USD is -0.00500 USD.
    let fills = [realized("venue-a", "aaa", -1_000)];
    let today = Today { realized: &fills, charged: &[], account: &[] };

    let loss = daily_loss_figure(
        Some(Amount::from_scaled(0)),
        Amount::from_scaled(0),
        today,
        &instruments(),
        &mut book,
    )
    .expect("measured");

    assert_eq!(loss.figure, (Scope::Portfolio, LimitKind::DailyLoss, money(500_000)));

    let mut unrated = Conversion::new(Pricing::single(&NO_MARKS, &NO_CATALOGUE), Some("usd"));
    let refused = daily_loss_figure(
        Some(Amount::from_scaled(0)),
        Amount::from_scaled(0),
        today,
        &instruments(),
        &mut unrated,
    );
    assert!(matches!(refused, Err(Unmeasured::Unconverted(_))), "{refused:?}");
}

/// A daily account fee paid is part of the loss. A credit is left out and counted.
#[test]
fn an_account_charge_paid_today_adds_to_the_loss() {
    let account = [
        crate::orders::AccountCharged {
            venue: "venue-a".into(),
            fee: bijective_lifecycle::Fee { asset: "usdt".into(), amount_scaled: 2, scale: 2 },
            at_ns: 1,
        },
        crate::orders::AccountCharged {
            venue: "venue-a".into(),
            fee: bijective_lifecycle::Fee { asset: "usdt".into(), amount_scaled: -5, scale: 2 },
            at_ns: 1,
        },
    ];
    let today = Today { realized: &[], charged: &[], account: &account };

    let loss = daily_loss_figure(
        Some(Amount::from_scaled(0)),
        Amount::from_scaled(0),
        today,
        &instruments(),
        &mut single(),
    )
    .expect("measured");

    assert_eq!(loss.figure, (Scope::Portfolio, LimitKind::DailyLoss, money(2_000_000)));
    assert_eq!(loss.fees_not_valued, 1);
}
