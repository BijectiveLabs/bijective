// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use bijective_venue::profile::{
    ObservedCurrencies, ObservedMinimums, ObservedPrecision, PrecisionRules, PriceValidity,
    VenueSymbol,
};

fn observed(settlement: Option<&str>, minimums: bool) -> ObservedBehaviour {
    let symbol = VenueSymbol::new("ABCUSDT");
    ObservedBehaviour {
        precision: Some(ObservedPrecision {
            symbol: symbol.clone(),
            rules: PrecisionRules {
                price_scale: 2,
                quantity_scale: 3,
                price_tick: 10,
                quantity_lot: 1,
                price_validity: PriceValidity::IncrementOnly,
            },
        }),
        minimums: minimums.then(|| ObservedMinimums {
            symbol: symbol.clone(),
            minimum_size: 1,
            minimum_notional: 500,
        }),
        currencies: Some(ObservedCurrencies {
            symbol,
            base: "ABC".into(),
            quote: settlement.map(str::to_string),
            settlement: settlement.map(str::to_string),
        }),
        ..Default::default()
    }
}

#[test]
fn an_instrument_takes_the_venues_precision_minimums_and_currencies() {
    let mut notes = Vec::new();
    let drafted =
        instrument_from("venue", "ABCUSDT", &observed(Some("USDT"), true), None, &mut notes);

    let instrument = drafted.expect("drafted");
    assert_eq!(instrument.id, InstrumentId::new("venue.ABCUSDT"));
    assert_eq!(instrument.precision.price_tick, 10);
    assert_eq!((instrument.minimum_size, instrument.minimum_notional), (1, 500));
    assert_eq!(instrument.settlement_currency, "usdt", "assets are named in lowercase");
    assert_eq!(instrument.base_currency.as_deref(), Some("abc"));
    assert!(notes.is_empty());
}

#[test]
fn an_unstated_settlement_is_refused_unless_named() {
    let mut notes = Vec::new();
    let unnamed = instrument_from("venue", "ABCUSDT", &observed(None, true), None, &mut notes);
    let named =
        instrument_from("venue", "ABCUSDT", &observed(None, true), Some("USDC"), &mut notes);

    assert!(matches!(unnamed, Err(DraftError::NoSettlement { .. })));
    let named = named.expect("named");
    assert_eq!(
        (named.settlement_currency.as_str(), named.quote_currency.as_str()),
        ("usdc", "usdc")
    );
}

#[test]
fn unpublished_minimums_are_the_lot_and_said() {
    let mut notes = Vec::new();
    let drafted =
        instrument_from("venue", "ABCUSDT", &observed(Some("usdt"), false), None, &mut notes);

    let instrument = drafted.expect("drafted");
    assert_eq!((instrument.minimum_size, instrument.minimum_notional), (1, 0));
    assert_eq!(notes.len(), 1);
}

#[test]
fn no_precision_is_refused() {
    let mut notes = Vec::new();
    let mut blank = observed(Some("usdt"), true);
    blank.precision = None;

    let drafted = instrument_from("venue", "ABCUSDT", &blank, None, &mut notes);
    assert!(matches!(drafted, Err(DraftError::NoPrecision { .. })));
}

#[test]
fn a_draft_is_unverified_and_reads_back() {
    let mut notes = Vec::new();
    let instrument =
        instrument_from("venue", "ABCUSDT", &observed(Some("usdt"), true), None, &mut notes)
            .expect("drafted");
    let profile = draft("venue", vec![instrument], None).expect("profile");

    assert!(profile.verified_at.is_none());
    let reread = VenueProfile::from_json(&profile.to_json()).expect("reads back");
    assert_eq!(reread.instruments.iter().count(), 1);
    assert!(matches!(draft("venue", Vec::new(), None), Err(DraftError::NoInstrument)));
}
