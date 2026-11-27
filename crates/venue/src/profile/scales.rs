// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The decimal places each instrument's prices and quantities are written in, read from a venue
//! profile's catalogue by venue symbol (decision 0007). One venue lists instruments of different
//! precision, so no figure here is per venue.

use std::collections::BTreeMap;

use bijective_model::VenueSymbol;

use crate::normalise::{Scale, ScaleError};
use crate::profile::instrument::InstrumentCatalogue;

/// One instrument's price and quantity scales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scales {
    pub price: Scale,
    pub quantity: Scale,
}

/// Every catalogued instrument's scales, keyed by its venue symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstrumentScales {
    by_symbol: BTreeMap<VenueSymbol, Scales>,
}

/// Why a catalogue gave no usable scales.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalesError {
    /// A declared scale this build cannot carry.
    Unusable { symbol: VenueSymbol, source: ScaleError },
    /// One pair was asked for and two instruments differ.
    Mixed { first: VenueSymbol, other: VenueSymbol },
    /// One pair was asked for and the catalogue lists nothing.
    Empty,
}

impl core::fmt::Display for ScalesError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unusable { symbol, source } => write!(f, "{}: {source}", symbol.as_str()),
            Self::Mixed { first, other } => write!(
                f,
                "{} and {} are written at different scales, and this reader takes one; give \
                 each precision its own feed or node",
                first.as_str(),
                other.as_str()
            ),
            Self::Empty => write!(f, "the profile lists no instrument to take a scale from"),
        }
    }
}

impl std::error::Error for ScalesError {}

impl InstrumentScales {
    /// Precondition: none. Postcondition: every catalogued symbol has its declared pair.
    pub fn of_catalogue(catalogue: &InstrumentCatalogue) -> Result<Self, ScalesError> {
        let mut by_symbol = BTreeMap::new();
        for instrument in catalogue.iter() {
            let unusable =
                |source| ScalesError::Unusable { symbol: instrument.symbol.clone(), source };
            let scales = Scales {
                price: Scale::new(instrument.precision.price_scale).map_err(unusable)?,
                quantity: Scale::new(instrument.precision.quantity_scale).map_err(unusable)?,
            };
            by_symbol.insert(instrument.symbol.clone(), scales);
        }
        Ok(Self { by_symbol })
    }

    /// The same pair for every listed symbol. For probes and fixtures that name their own.
    pub fn uniform_over<'a>(symbols: impl IntoIterator<Item = &'a str>, scales: Scales) -> Self {
        let by_symbol = symbols.into_iter().map(|s| (VenueSymbol::new(s), scales)).collect();
        Self { by_symbol }
    }

    /// `None` for a symbol outside the catalogue.
    #[must_use]
    pub fn of(&self, symbol: &VenueSymbol) -> Option<Scales> {
        self.by_symbol.get(symbol).copied()
    }

    /// The one pair every instrument shares, for a reader that writes a single scale.
    pub fn uniform(&self) -> Result<Scales, ScalesError> {
        let mut entries = self.by_symbol.iter();
        let Some((first, scales)) = entries.next() else { return Err(ScalesError::Empty) };
        match entries.find(|(_, other)| *other != scales) {
            Some((other, _)) => {
                Err(ScalesError::Mixed { first: first.clone(), other: other.clone() })
            }
            None => Ok(*scales),
        }
    }
}

impl FromIterator<(VenueSymbol, Scales)> for InstrumentScales {
    fn from_iter<I: IntoIterator<Item = (VenueSymbol, Scales)>>(pairs: I) -> Self {
        Self { by_symbol: pairs.into_iter().collect() }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/profile/scales_tests.rs"]
mod tests;
