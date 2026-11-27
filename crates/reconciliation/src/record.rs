// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The two sides, deliberately not the same type.
//!
//! No conversion between them in either direction.
use bijective_model::VenueSymbol;

/// What the platform records as held, as of a sequence position in our own stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPosition {
    pub instrument: VenueSymbol,
    pub client_order_id: String,
    pub venue_order_id: Option<String>,
    pub quantity_scaled: i128,
    pub average_price_scaled: i128,
    /// When our record came into being. Used only to judge the in-flight window.
    pub opened_at_ns: i128,
}

/// What the venue says it holds. Never assigned into ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenuePosition {
    pub instrument: VenueSymbol,
    pub client_order_id: Option<String>,
    pub venue_order_id: Option<String>,
    pub quantity_scaled: i128,
    pub average_price_scaled: i128,
    pub updated_at_ms: u64,
}

impl VenuePosition {
    /// Reads the venue's own account into the venue side. A position carries no order identifiers.
    pub fn from_account(position: &bijective_model::PositionSnapshot, updated_at_ms: u64) -> Self {
        Self {
            instrument: position.instrument.clone(),
            client_order_id: None,
            venue_order_id: None,
            quantity_scaled: position.quantity_scaled,
            average_price_scaled: position.entry_price_scaled,
            updated_at_ms,
        }
    }
}

/// How a venue's records may be paired with ours. Read from the venue assurance profile, never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// The venue's own identifiers are reliable.
    VenueIdentifier,
    /// They are not, and the profile declares what to use instead.
    Fallback(FallbackKey),
    /// Nothing is declared, so nothing may be paired.
    Undeclared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackKey {
    ClientOrderId,
    /// The weakest declared pairing: one open position per instrument.
    Instrument,
}
