// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The profile's instrument rules reach the store once per change.

use bijective_model::InstrumentId;
use bijective_serialization::signal::instruments::standing;
use bijective_venue::contract::VenueSymbol;
use bijective_venue::profile::{
    Instrument, InstrumentCatalogue, InstrumentKind, InstrumentStatus, PrecisionRules,
    PriceValidity,
};

use super::*;

fn catalogue(tick: i128, status: InstrumentStatus) -> InstrumentCatalogue {
    InstrumentCatalogue::from_instruments([Instrument {
        symbol: VenueSymbol::new("AAA"),
        id: InstrumentId::new("btc-perp"),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: None,
        precision: PrecisionRules {
            price_scale: 1,
            quantity_scale: 3,
            price_tick: tick,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 50,
        status,
    }])
}

#[test]
fn the_rules_are_declared_when_they_change_and_not_again_when_they_do_not() {
    let store = std::env::temp_dir().join(format!("bl-declared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store);
    std::fs::create_dir_all(&store).expect("store");

    let first = catalogue(5, InstrumentStatus::Trading);
    assert!(declare_instruments(&store, "venue-a", &first).expect("declare"));
    assert!(!declare_instruments(&store, "venue-a", &first).expect("declare"), "nothing changed");
    let held = standing(&store, "venue-a").expect("read").expect("declared");
    assert_eq!(held.instruments[0].instrument, "btc-perp");
    assert_eq!((held.instruments[0].price_tick, held.instruments[0].trading), (5, true));

    let halted = catalogue(5, InstrumentStatus::Halted);
    assert!(declare_instruments(&store, "venue-a", &halted).expect("declare"));
    assert!(!standing(&store, "venue-a").expect("read").expect("declared").instruments[0].trading);
}
