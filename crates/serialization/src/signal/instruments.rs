// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The rules a venue sets for each instrument it lists, recorded where the engines that may not
//! read a venue profile can read them.
//!
//! The data engine loads the venue's profile and writes these rules when they differ from the
//! standing record. The strategy engine sends them to a strategy before its first top, so a price
//! or quantity can be placed on the venue's increments before a proposal is made, not refused after.

use std::path::Path;

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_event_store::store::{StoreError, read_range};

/// Version 2 adds what the instrument is and what it is quoted and settled in. Version 3 adds the
/// asset a quantity is counted in. Older declarations are still read, with those absent.
pub const VERSION: u16 = 3;

/// The stream one venue's instrument rules are declared on. One per venue, because each venue's
/// data engine writes its own and a stream has one writer.
#[must_use]
pub fn stream_for(venue: &str) -> String {
    format!("venue-instruments-{venue}")
}

/// One instrument as the venue lists it. Figures are at the instrument's own scales.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentRules {
    /// The platform's identity for the instrument.
    pub instrument: String,
    pub price_scale: u8,
    pub quantity_scale: u8,
    /// Smallest price increment, in `price_scale` units.
    pub price_tick: i128,
    /// Smallest quantity increment, in `quantity_scale` units.
    pub quantity_lot: i128,
    /// Smallest quantity accepted, in `quantity_scale` units.
    pub minimum_size: i128,
    /// Smallest order value accepted, in `price_scale` units.
    pub minimum_notional: i128,
    /// Whether the venue accepts orders for it now.
    pub trading: bool,
    /// What the instrument is, as the venue profile names it (`spot`, `perpetual`, ...). Text, so
    /// a kind a later venue lists reaches a strategy without a change to this record.
    pub kind: Option<String>,
    /// The currency a price is stated in.
    pub quote_currency: Option<String>,
    /// The currency profit and loss settle in. Differs from the quote on an inverse contract.
    pub settlement_currency: Option<String>,
    /// The asset a quantity is counted in.
    pub base_currency: Option<String>,
}

/// Every instrument one venue lists, in identity order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueInstruments {
    pub venue: String,
    pub instruments: Vec<InstrumentRules>,
}

#[derive(Debug)]
pub enum InstrumentsError {
    Encode(EncodeError),
    Decode(DecodeError),
    Store(StoreError),
}

impl core::fmt::Display for InstrumentsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Encode(e) => write!(f, "the instrument rules could not be encoded: {e:?}"),
            Self::Decode(e) => write!(f, "the instrument rules could not be read: {e:?}"),
            Self::Store(e) => write!(f, "store: {e:?}"),
        }
    }
}

impl std::error::Error for InstrumentsError {}

pub fn encode(rules: &VenueInstruments) -> Result<Vec<u8>, InstrumentsError> {
    let mut w = Writer::new(ClassId::VenueInstrumentsDeclared, VERSION);
    w.str(&rules.venue).map_err(InstrumentsError::Encode)?;
    w.u64(rules.instruments.len() as u64);
    for i in &rules.instruments {
        w.str(&i.instrument).map_err(InstrumentsError::Encode)?;
        w.u8(i.price_scale).u8(i.quantity_scale);
        w.i128(i.price_tick).i128(i.quantity_lot).i128(i.minimum_size).i128(i.minimum_notional);
        w.u8(u8::from(i.trading));
        for text in [&i.kind, &i.quote_currency, &i.settlement_currency, &i.base_currency] {
            match text {
                Some(text) => w.u8(1).str(text).map_err(InstrumentsError::Encode)?,
                None => w.u8(0),
            };
        }
    }
    Ok(w.finish())
}

pub fn decode(payload: &[u8]) -> Result<VenueInstruments, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::VenueInstrumentsDeclared {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    let venue = r.str()?.to_string();
    let count = r.u64()?;
    let mut instruments = Vec::new();
    for _ in 0..count {
        let instrument = r.str()?.to_string();
        let (price_scale, quantity_scale) = (r.u8()?, r.u8()?);
        let (price_tick, quantity_lot) = (r.i128()?, r.i128()?);
        let (minimum_size, minimum_notional) = (r.i128()?, r.i128()?);
        let trading = match r.u8()? {
            0 => false,
            1 => true,
            other => {
                return Err(DecodeError::InvalidValue { field: "trading", found: other.into() });
            }
        };
        let mut declared = [None, None, None, None];
        // Version 2 carries the first three, version 3 the fourth.
        let carried = match envelope.version {
            0 | 1 => 0,
            2 => 3,
            _ => 4,
        };
        for slot in declared.iter_mut().take(carried) {
            *slot = optional_text(&mut r)?;
        }
        let [kind, quote_currency, settlement_currency, base_currency] = declared;
        instruments.push(InstrumentRules {
            instrument,
            price_scale,
            quantity_scale,
            price_tick,
            quantity_lot,
            minimum_size,
            minimum_notional,
            trading,
            kind,
            quote_currency,
            settlement_currency,
            base_currency,
        });
    }
    r.finish()?;
    Ok(VenueInstruments { venue, instruments })
}

fn optional_text(r: &mut Reader<'_>) -> Result<Option<String>, DecodeError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(r.str()?.to_string())),
        other => Err(DecodeError::InvalidValue { field: "declared text", found: other.into() }),
    }
}

/// The latest rules declared for `venue`, or none. Nothing is deleted, so a run made under earlier
/// rules can still be read against them.
pub fn standing(root: &Path, venue: &str) -> Result<Option<VenueInstruments>, InstrumentsError> {
    let entries =
        read_range(root, &stream_for(venue), 0, u64::MAX).map_err(InstrumentsError::Store)?;
    let mut found = None;
    for entry in entries {
        // Refused, never skipped: the unreadable entry may be the standing declaration.
        let rules = decode(&entry.payload).map_err(InstrumentsError::Decode)?;
        if rules.venue == venue {
            found = Some(rules);
        }
    }
    Ok(found)
}

#[cfg(test)]
#[path = "../../tests/unit/signal/instruments_tests.rs"]
mod tests;
