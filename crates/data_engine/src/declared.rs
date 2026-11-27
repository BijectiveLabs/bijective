// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The venue profile's instrument rules, declared into the store for engines that may not read a
//! profile.

use std::path::Path;

use bijective_event_store::store::StreamWriter;
use bijective_serialization::signal::instruments::{
    InstrumentRules, InstrumentsError, VenueInstruments, encode, standing, stream_for,
};
use bijective_venue::profile::{InstrumentCatalogue, InstrumentKind};

/// The rules `catalogue` sets, in symbol order.
#[must_use]
pub fn rules_of(venue: &str, catalogue: &InstrumentCatalogue) -> VenueInstruments {
    let instruments = catalogue
        .iter()
        .map(|i| InstrumentRules {
            instrument: i.id.as_str().to_string(),
            price_scale: i.precision.price_scale,
            quantity_scale: i.precision.quantity_scale,
            price_tick: i.precision.price_tick,
            quantity_lot: i.precision.quantity_lot,
            minimum_size: i.minimum_size,
            minimum_notional: i.minimum_notional,
            trading: i.status.admits_orders(),
            kind: Some(kind_name(i.kind).to_string()),
            quote_currency: Some(i.quote_currency.clone()),
            settlement_currency: Some(i.settlement_currency.clone()),
            base_currency: i.base_currency.clone(),
        })
        .collect();
    VenueInstruments { venue: venue.to_string(), instruments }
}

/// The profile's own name for the kind, so a strategy reads what an operator reads.
fn kind_name(kind: InstrumentKind) -> &'static str {
    match kind {
        InstrumentKind::Perpetual => "perpetual",
        InstrumentKind::Spot => "spot",
        InstrumentKind::DatedFuture => "dated-future",
        InstrumentKind::Inverse => "inverse",
        InstrumentKind::Outcome => "outcome",
        InstrumentKind::Equity => "equity",
    }
}

/// Writes the rules when they differ from the standing declaration, so a restart with the same
/// profile adds nothing. Returns whether a declaration was written.
pub fn declare_instruments(
    store: &Path,
    venue: &str,
    catalogue: &InstrumentCatalogue,
) -> Result<bool, InstrumentsError> {
    let rules = rules_of(venue, catalogue);
    if standing(store, venue)?.as_ref() == Some(&rules) {
        return Ok(false);
    }
    let mut writer =
        StreamWriter::open(store, &stream_for(venue)).map_err(InstrumentsError::Store)?;
    writer.append(&encode(&rules)?).map_err(InstrumentsError::Store)?;
    Ok(true)
}

#[cfg(test)]
#[path = "../tests/unit/declared_tests.rs"]
mod tests;
