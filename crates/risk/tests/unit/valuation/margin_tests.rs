// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use std::collections::BTreeMap;

use bijective_limits::{Amount, LimitKind, Ratio, Scope, Threshold};
use bijective_model::AccountMargin;

use super::*;
use crate::valuation::Pricing;
use crate::valuation::currency::{Conversion, Rates};

fn usd(equity: i128, maintenance: i128) -> Option<AccountMargin> {
    Some(AccountMargin {
        currency: "USD".into(),
        equity_scaled: equity,
        maintenance_scaled: maintenance,
        scale: 2,
    })
}

fn figures(
    margins: &[(String, Option<AccountMargin>)],
    gross: i128,
) -> Result<[(Scope, LimitKind, Threshold); 2], MarginUnmeasured> {
    let marks = BTreeMap::new();
    let catalogue = BTreeMap::new();
    let pricing =
        Pricing { marks: &marks, catalogue: &catalogue, rates: Rates::single(), as_of_ns: 1 };
    let mut book = Conversion::new(pricing, Some("usd"));
    margin_figures(margins, Amount::from_scaled(gross), &mut book)
}

fn ratio_of(figure: &(Scope, LimitKind, Threshold)) -> i128 {
    match figure.2 {
        Threshold::Ratio(r) => r.scaled(),
        Threshold::Money(_) => panic!("a ratio"),
    }
}

/// Two venues' equity is summed before the gross is taken over it: 30 000 over 10 000 is three.
#[test]
fn leverage_is_the_gross_over_every_venues_equity() {
    let margins = vec![
        ("venue-a".to_string(), usd(600_000, 3_000)),
        ("venue-b".to_string(), usd(400_000, 0)),
    ];
    let [leverage, margin_use] = figures(&margins, 30_000 * 100_000_000).expect("measured");
    assert_eq!((leverage.0.clone(), leverage.1), (Scope::Portfolio, LimitKind::Leverage));
    assert_eq!(ratio_of(&leverage), 3 * Ratio::ONE);
    assert_eq!(margin_use.1, LimitKind::MarginUse);
    assert_eq!(ratio_of(&margin_use), 30, "30 over 10 000 is 0.003, 30 at 10 000 to one");
}

#[test]
fn a_figure_is_rounded_up_never_down() {
    let margins = vec![("venue-a".to_string(), usd(300, 1))];
    let [leverage, margin_use] = figures(&margins, 100_000_000).expect("measured");
    assert_eq!(ratio_of(&leverage), 3_334);
    assert_eq!(ratio_of(&margin_use), 34);
}

#[test]
fn a_venue_without_margin_or_an_account_without_equity_is_unmeasured() {
    let silent = vec![("venue-a".to_string(), usd(100, 0)), ("other".to_string(), None)];
    assert_eq!(figures(&silent, 0), Err(MarginUnmeasured::NotStated { venue: "other".into() }));
    let empty = vec![("venue-a".to_string(), usd(0, 0))];
    assert_eq!(figures(&empty, 0), Err(MarginUnmeasured::NoEquity));
}

/// Equity in a currency the book has no rate for is not guessed at par.
#[test]
fn equity_in_another_currency_without_a_rate_is_unmeasured() {
    let mut eur = usd(100, 0);
    if let Some(m) = eur.as_mut() {
        m.currency = "eur".into();
    }
    let measured = figures(&[("venue".to_string(), eur)], 0);
    assert!(matches!(measured, Err(MarginUnmeasured::Unconverted(_))), "{measured:?}");
}

fn stated(venue: &str, currency: &str, equity_scaled: i128, scale: u8) -> StatedEquity {
    StatedEquity { venue: venue.into(), currency: currency.into(), equity_scaled, scale }
}

/// The book's equity sums each venue at its own scale. Zero and negative equity are figures.
#[test]
fn the_book_equity_sums_every_venue_at_its_own_scale() {
    let marks = BTreeMap::new();
    let catalogue = BTreeMap::new();
    let pricing =
        Pricing { marks: &marks, catalogue: &catalogue, rates: Rates::single(), as_of_ns: 1 };
    let mut book = Conversion::new(pricing, Some("usd"));
    let parts = [stated("venue-a", "USD", 1_050, 2), stated("venue-b", "usd", -50_000_000, 8)];
    let equity = book_equity(&parts, &mut book).expect("measured");
    assert_eq!(equity, Amount::from_scaled(10 * 100_000_000), "10.50 less 0.50");
    let mut book = Conversion::new(pricing, Some("usd"));
    let foreign = book_equity(&[stated("venue-a", "eur", 1, 0)], &mut book);
    assert!(matches!(foreign, Err(MarginUnmeasured::Unconverted(_))), "{foreign:?}");
}
