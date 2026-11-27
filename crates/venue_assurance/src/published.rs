// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A venue profile drafted from what the venue publishes, so recording or trading a market starts
//! without a hand-written profile. The draft is unverified: it says only what the venue's
//! instrument answer states, and what that answer leaves out is refused or stated, never guessed.

use std::collections::BTreeMap;

use bijective_venue::profile::{
    DataClass, FundingModel, IdentifierRules, Instrument, InstrumentCatalogue, InstrumentId,
    InstrumentKind, InstrumentStatus, ObservedBehaviour, ReduceOnly, SupportDepth, SupportEntry,
    Transport, TransportProfile, VenueProfile, declaration::PROFILE_FORMAT,
};

/// How stale a book may be before the profile calls the stream silent: the node's freshness bound.
const MAX_STALENESS_NS: i128 = 5_000_000_000;

/// Why an observation cannot be drafted into an instrument. Each names what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftError {
    /// The answer stated no precision: a price or quantity scale cannot be stood behind.
    NoPrecision {
        instrument: String,
    },
    /// The answer stated no settlement currency and none was given.
    NoSettlement {
        instrument: String,
    },
    NoInstrument,
    Support {
        detail: String,
    },
}

impl core::fmt::Display for DraftError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPrecision { instrument } => {
                write!(f, "{instrument}: the venue's answer states no precision")
            }
            Self::NoSettlement { instrument } => write!(
                f,
                "{instrument}: the venue's answer states no quote or settlement currency; name \
                 it with --settles-in <asset>"
            ),
            Self::NoInstrument => write!(f, "a profile with no instrument declares nothing"),
            Self::Support { detail } => write!(f, "the support declaration was refused: {detail}"),
        }
    }
}

impl std::error::Error for DraftError {}

/// One instrument from its observation. Assets are named in lowercase, as the platform names
/// them. Minimums the venue does not publish are the lot and no notional floor, and `notes`
/// says so. The venue still refuses what it refuses.
pub fn instrument_from(
    venue: &str,
    instrument: &str,
    observed: &ObservedBehaviour,
    settles_in: Option<&str>,
    notes: &mut Vec<String>,
) -> Result<Instrument, DraftError> {
    let named = || instrument.to_string();
    let precision =
        observed.precision.as_ref().ok_or(DraftError::NoPrecision { instrument: named() })?;
    let currencies = observed.currencies.as_ref();
    let settlement = currencies
        .and_then(|c| c.settlement.clone())
        .or_else(|| settles_in.map(str::to_string))
        .ok_or(DraftError::NoSettlement { instrument: named() })?
        .to_lowercase();
    let quote = currencies
        .and_then(|c| c.quote.clone())
        .map_or_else(|| settlement.clone(), |q| q.to_lowercase());
    let (minimum_size, minimum_notional) = match &observed.minimums {
        Some(m) => (m.minimum_size, m.minimum_notional),
        None => {
            notes.push(format!(
                "{instrument}: no minimums published; the lot is the smallest order and no \
                 notional floor is declared"
            ));
            (precision.rules.quantity_lot, 0)
        }
    };
    Ok(Instrument {
        symbol: precision.symbol.clone(),
        // Venue-namespaced: two venues may publish the same symbol.
        id: InstrumentId::new(format!("{venue}.{instrument}")),
        // Every instrument answer this build reads is the venue's perpetual listing.
        kind: InstrumentKind::Perpetual,
        settlement_currency: settlement,
        quote_currency: quote,
        base_currency: currencies.map(|c| c.base.to_lowercase()),
        precision: precision.rules.clone(),
        minimum_size,
        minimum_notional,
        status: InstrumentStatus::Trading,
    })
}

/// The profile for `instruments`, unverified. Funding is the first instrument's as observed, or
/// unpublished. Client identifiers keep to lowercase letters, digits and hyphens within 36
/// characters, a subset every registered venue accepts and the form the node assigns.
pub fn draft(
    venue: &str,
    instruments: Vec<Instrument>,
    funding: Option<FundingModel>,
) -> Result<VenueProfile, DraftError> {
    if instruments.is_empty() {
        return Err(DraftError::NoInstrument);
    }
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: funding.unwrap_or(FundingModel::UnpublishedOnThisTransport),
            data_classes: vec![DataClass::TopOfBook, DataClass::Depth],
            max_staleness_ns: Some(MAX_STALENESS_NS),
        },
    );
    let support = SupportEntry::declare(venue, SupportDepth::Full, true)
        .map_err(|e| DraftError::Support { detail: e.to_string() })?;
    Ok(VenueProfile {
        format: PROFILE_FORMAT,
        venue: venue.to_string(),
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
        support,
    })
}

#[cfg(test)]
#[path = "../tests/unit/published_tests.rs"]
mod tests;
