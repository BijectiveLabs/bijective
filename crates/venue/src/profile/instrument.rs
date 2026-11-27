// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The instrument catalogue, read from a venue and verified against it, never authored.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use bijective_model::{InstrumentId, VenueSymbol};

use crate::profile::declaration::PrecisionRules;

/// Declared, never inferred: a symbol's shape is only a naming convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstrumentKind {
    Perpetual,
    Spot,
    DatedFuture,
    /// Margined and settled in the base asset, so position value moves non-linearly in price.
    Inverse,
    /// A share in one outcome of a question, paying one unit of collateral if it resolves true
    /// and nothing otherwise. Priced in `(0, 1)` and held long only
    /// (`docs/decisions/0002-prediction-market-outcomes.md`).
    Outcome,
    /// A share of a listed company, priced in its quote currency and valued like a spot holding.
    /// Held long only (`docs/decisions/0004-multi-asset-equities.md`).
    Equity,
}

impl InstrumentKind {
    /// The kinds this platform trades: a spot pair, a linear perpetual, an outcome share and an
    /// equity. A dated future has an
    /// expiry and a settlement the platform does not handle, and an inverse contract values
    /// non-linearly, so both are refused at admission rather than valued as if they were linear.
    #[must_use]
    pub const fn is_tradable(self) -> bool {
        matches!(
            self,
            InstrumentKind::Spot
                | InstrumentKind::Perpetual
                | InstrumentKind::Outcome
                | InstrumentKind::Equity
        )
    }

    /// Whether a position may be short. A spot asset, an outcome share or an equity is sold only
    /// from what is held, so a sell beyond the holding is refused before it is sent.
    #[must_use]
    pub const fn can_be_short(self) -> bool {
        matches!(self, InstrumentKind::Perpetual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstrumentStatus {
    Trading,
    Halted,
    Delisted,
}

impl InstrumentStatus {
    #[must_use]
    pub fn admits_orders(self) -> bool {
        matches!(self, InstrumentStatus::Trading)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instrument {
    #[serde(with = "text")]
    pub symbol: VenueSymbol,
    #[serde(with = "text")]
    pub id: InstrumentId,
    pub kind: InstrumentKind,
    /// Two currencies because they differ on an inverse contract. One field would be quietly
    /// wrong there.
    pub settlement_currency: String,
    pub quote_currency: String,
    /// The asset a quantity is counted in: what one unit of the spot pair or perpetual is a unit
    /// of. Absent on a profile written before it was declared, which is unknown, not none, so
    /// nothing that needs it is measured from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_currency: Option<String>,
    pub precision: PrecisionRules,
    /// Smallest quantity the venue accepts, in `precision.quantity_scale` units.
    #[serde(with = "crate::profile::wide")]
    pub minimum_size: i128,
    /// Smallest order value, in `precision.price_scale` units. Size and notional are both enforced
    /// and refuse different orders.
    #[serde(with = "crate::profile::wide")]
    pub minimum_notional: i128,
    pub status: InstrumentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueError {
    /// Refused, not defaulted: no precision could be stood behind.
    UnknownSymbol,
}

/// Every instrument a venue publishes, keyed by the venue's own symbol.
/// Transparent, so a profile carries a list rather than an object wrapping one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstrumentCatalogue {
    #[serde(with = "keyed")]
    entries: BTreeMap<VenueSymbol, Instrument>,
}

/// Plain-text serde for the core identities: `crates/model` declares no dependencies, so the
/// conversion lives at this boundary.
mod text {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub trait Text: Sized {
        fn text(&self) -> &str;
        fn from_text(value: String) -> Self;
    }

    impl Text for bijective_model::VenueSymbol {
        fn text(&self) -> &str {
            self.as_str()
        }
        fn from_text(value: String) -> Self {
            Self::new(value)
        }
    }

    impl Text for bijective_model::InstrumentId {
        fn text(&self) -> &str {
            self.as_str()
        }
        fn from_text(value: String) -> Self {
            Self::new(value)
        }
    }

    pub fn serialize<T: Text, S: Serializer>(value: &T, s: S) -> Result<S::Ok, S::Error> {
        value.text().serialize(s)
    }

    pub fn deserialize<'de, T: Text, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        Ok(T::from_text(String::deserialize(d)?))
    }
}

/// Serialised as a list: each entry carries its key, and writing it twice lets a file disagree
/// with itself.
mod keyed {
    use std::collections::BTreeMap;

    use bijective_model::VenueSymbol;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Instrument;

    pub fn serialize<S: Serializer>(
        value: &BTreeMap<VenueSymbol, Instrument>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value.values().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<VenueSymbol, Instrument>, D::Error> {
        Ok(Vec::<Instrument>::deserialize(d)?.into_iter().map(|i| (i.symbol.clone(), i)).collect())
    }
}

impl InstrumentCatalogue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_instruments(items: impl IntoIterator<Item = Instrument>) -> Self {
        let mut catalogue = Self::new();
        for item in items {
            catalogue.record(item);
        }
        catalogue
    }

    pub fn record(&mut self, instrument: Instrument) {
        self.entries.insert(instrument.symbol.clone(), instrument);
    }

    pub fn get(&self, symbol: &VenueSymbol) -> Result<&Instrument, CatalogueError> {
        self.entries.get(symbol).ok_or(CatalogueError::UnknownSymbol)
    }

    /// Per instrument, never per venue: venue-wide precision validated all but one instrument
    /// against the wrong granularity (fixed 2026-08-25).
    pub fn precision(&self, symbol: &VenueSymbol) -> Result<&PrecisionRules, CatalogueError> {
        self.get(symbol).map(|i| &i.precision)
    }

    /// Every instrument this venue publishes, in symbol order, so cross-venue merges reproduce.
    pub fn iter(&self) -> impl Iterator<Item = &Instrument> {
        self.entries.values()
    }

    /// Several symbols per identity is the ordinary case. Risk scopes key by identity.
    pub fn symbols_for(&self, id: &InstrumentId) -> Vec<&VenueSymbol> {
        self.entries.values().filter(|i| &i.id == id).map(|i| &i.symbol).collect()
    }

    pub fn identity_of(&self, symbol: &VenueSymbol) -> Result<&InstrumentId, CatalogueError> {
        self.get(symbol).map(|i| &i.id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/profile/instrument_tests.rs"]
mod tests;
