// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

fn perp(symbol: &str, id: &str, tick: i128, status: InstrumentStatus) -> Instrument {
    Instrument {
        symbol: VenueSymbol::new(symbol),
        id: InstrumentId::new(id),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: None,
        precision: PrecisionRules {
            price_scale: 2,
            quantity_scale: 3,
            price_tick: tick,
            quantity_lot: 1,
            price_validity: crate::profile::declaration::PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 100,
        status,
    }
}

/// Two instruments on one venue differ. One precision per venue was a live defect.
#[test]
fn two_instruments_on_one_venue_carry_different_precision() {
    let c = InstrumentCatalogue::from_instruments([
        perp("aaa", "aaa", 10, InstrumentStatus::Trading),
        perp("bbb", "bbb", 1, InstrumentStatus::Trading),
    ]);

    assert_eq!(c.precision(&VenueSymbol::new("aaa")).unwrap().price_tick, 10);
    assert_eq!(c.precision(&VenueSymbol::new("bbb")).unwrap().price_tick, 1);
}

#[test]
fn an_unknown_symbol_yields_no_precision() {
    let c =
        InstrumentCatalogue::from_instruments([perp("aaa", "aaa", 10, InstrumentStatus::Trading)]);

    assert_eq!(c.precision(&VenueSymbol::new("ccc")), Err(CatalogueError::UnknownSymbol));
}

/// One identity, two venue symbols - the case a risk scope keyed by symbol gets wrong.
#[test]
fn one_identity_gathers_every_symbol_that_carries_it() {
    let c = InstrumentCatalogue::from_instruments([
        perp("aaa", "btc-perp", 10, InstrumentStatus::Trading),
        perp("xbtusd", "btc-perp", 1, InstrumentStatus::Trading),
        perp("bbb", "eth-perp", 1, InstrumentStatus::Trading),
    ]);

    let carried = c.symbols_for(&InstrumentId::new("btc-perp"));
    assert_eq!(carried.len(), 2, "a limit on this exposure must bind both");
    assert_eq!(
        c.identity_of(&VenueSymbol::new("xbtusd")).unwrap(),
        &InstrumentId::new("btc-perp"),
        "nothing in the text says so; the mapping was declared"
    );
}

#[test]
fn only_a_trading_instrument_admits_orders() {
    assert!(InstrumentStatus::Trading.admits_orders());
    assert!(!InstrumentStatus::Halted.admits_orders());
    assert!(!InstrumentStatus::Delisted.admits_orders());
}
