// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Emits a venue profile for the drills. Names no venue, both arrive as arguments.
//! Generated and unverified on purpose: a verified profile comes from venue assurance's verifier.

use std::collections::BTreeMap;

use bijective_venue::profile::{
    DataClass, FundingModel, IdentifierRules, Instrument, InstrumentCatalogue, InstrumentId,
    InstrumentKind, InstrumentStatus, PrecisionRules, PriceValidity, ReduceOnly, SupportDepth,
    SupportEntry, Transport, TransportProfile, VenueProfile, VenueSymbol,
    declaration::PROFILE_FORMAT,
};

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(venue), Some(symbols)) = (args.next(), args.next()) else {
        eprintln!("usage: drill_profile <venue> <symbol,symbol,...>");
        return std::process::ExitCode::from(2);
    };

    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Published { interval_ns: 28_800_000_000_000 },
            data_classes: vec![DataClass::TopOfBook, DataClass::Depth],
            max_staleness_ns: Some(5_000_000_000),
        },
    );

    let instruments: Vec<Instrument> = symbols
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|symbol| Instrument {
            symbol: VenueSymbol::new(symbol),
            // Venue-namespaced: two venues may publish the same symbol.
            id: InstrumentId::new(format!("{venue}.{symbol}")),
            kind: InstrumentKind::Perpetual,
            settlement_currency: "usdt".into(),
            quote_currency: "usdt".into(),
            base_currency: None,
            precision: PrecisionRules {
                price_scale: 2,
                quantity_scale: 3,
                price_tick: 10,
                quantity_lot: 1,
                price_validity: PriceValidity::IncrementOnly,
            },
            minimum_size: 1,
            minimum_notional: 500,
            status: InstrumentStatus::Trading,
        })
        .collect();

    if instruments.is_empty() {
        eprintln!("a profile with no instrument declares nothing");
        return std::process::ExitCode::from(2);
    }

    let profile = VenueProfile {
        format: PROFILE_FORMAT,
        venue: venue.clone(),
        verified_at: None,
        transports,
        reduce_only: ReduceOnly::PlacementPrecondition,
        order_types: Vec::new(),
        instruments: InstrumentCatalogue::from_instruments(instruments),
        identifier_rules: IdentifierRules {
            allowed: "abcdefghijklmnopqrstuvwxyz0123456789-".into(),
            max_len: 36,
        },
        transformations: Vec::new(),
        support: match SupportEntry::declare(&venue, SupportDepth::Full, true) {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("the support declaration was refused: {e}");
                return std::process::ExitCode::FAILURE;
            }
        },
    };

    println!("{}", profile.to_json());
    std::process::ExitCode::SUCCESS
}
