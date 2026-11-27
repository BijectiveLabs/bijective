// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Scales per instrument: each symbol keeps its own pair, and a single pair is given only when
//! every instrument shares it.

use super::*;
use crate::profile::declaration::{PrecisionRules, PriceValidity};
use crate::profile::instrument::{Instrument, InstrumentKind, InstrumentStatus};
use bijective_model::InstrumentId;

fn perp(symbol: &str, price_scale: u8, quantity_scale: u8) -> Instrument {
    Instrument {
        symbol: VenueSymbol::new(symbol),
        id: InstrumentId::new(symbol),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: None,
        precision: PrecisionRules {
            price_scale,
            quantity_scale,
            price_tick: 1,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 1,
        status: InstrumentStatus::Trading,
    }
}

fn pair(price: u8, quantity: u8) -> Scales {
    Scales { price: Scale::new(price).unwrap(), quantity: Scale::new(quantity).unwrap() }
}

#[test]
fn each_instrument_keeps_its_own_scales() {
    let catalogue =
        InstrumentCatalogue::from_instruments([perp("BTCUSDT", 2, 3), perp("ETHUSDT", 2, 2)]);
    let scales = InstrumentScales::of_catalogue(&catalogue).unwrap();
    assert_eq!(scales.of(&VenueSymbol::new("BTCUSDT")), Some(pair(2, 3)));
    assert_eq!(scales.of(&VenueSymbol::new("ETHUSDT")), Some(pair(2, 2)));
    assert_eq!(scales.of(&VenueSymbol::new("SOLUSDT")), None);
    assert!(
        matches!(scales.uniform(), Err(ScalesError::Mixed { .. })),
        "a single-scale reader is refused, not handed one of the two"
    );
}

#[test]
fn a_shared_pair_is_given_and_an_empty_or_unusable_catalogue_refused() {
    let shared = InstrumentCatalogue::from_instruments([perp("AAA", 4, 0), perp("BBB", 4, 0)]);
    assert_eq!(InstrumentScales::of_catalogue(&shared).unwrap().uniform(), Ok(pair(4, 0)));
    assert_eq!(InstrumentScales::default().uniform(), Err(ScalesError::Empty));
    let wide = InstrumentCatalogue::from_instruments([perp("AAA", 19, 0)]);
    assert!(matches!(InstrumentScales::of_catalogue(&wide), Err(ScalesError::Unusable { .. })));
}
