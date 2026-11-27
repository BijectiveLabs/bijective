// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue profiles: the data an adapter reads and the declaration derived from it.

pub mod constraints;
pub mod declaration;
pub mod funding;
pub mod instrument;
pub mod scales;
pub mod support;
pub mod wide;

pub use bijective_model::{InstrumentId, VenueSymbol};
pub use constraints::{IdentifierRejection, IdentifierRules, Transformation};
pub use declaration::{
    CapabilityDeclaration, DataClass, IncrementRejection, ObservedBehaviour, ObservedCurrencies,
    OrderTypeSupport, PrecisionRules, ProfileError, ReduceOnly, TransportProfile, VenueProfile,
    VerificationStamp,
};
pub use declaration::{ObservedMinimums, ObservedPrecision, PriceValidity};
pub use funding::{FundingFigure, FundingModel, Transport};
pub use instrument::{
    CatalogueError, Instrument, InstrumentCatalogue, InstrumentKind, InstrumentStatus,
};

pub use scales::{InstrumentScales, Scales, ScalesError};
pub use support::{SupportDepth, SupportEntry, SupportError};

#[cfg(test)]
#[path = "../../tests/unit/profile/tests.rs"]
mod tests;
