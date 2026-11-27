// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reads the venue's published perpetual metadata into an observation for venue assurance.
//! Design notes and sources: `docs/integrations/hyperliquid.md` (Tick and lot, Perpetuals
//! metadata, read 2026-11-15).

use std::time::Duration;

use bijective_venue::profile::{
    ObservedBehaviour, ObservedCurrencies, ObservedPrecision, PrecisionRules, PriceValidity,
    VenueSymbol,
};

/// A read that has not answered in this long is a failed observation, never an empty one.
const TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Public metadata. Needs no credential.
const PATH_INFO: &str = "/info";
const META_BODY: &str = "{\"type\":\"meta\"}";

/// Price decimals for a perpetual are this less the size decimals (Tick and lot).
const PERP_MAX_DECIMALS: u8 = 6;
/// Prices carry at most five significant figures, integers excepted (Tick and lot).
const SIGNIFICANT_FIGURES: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveError {
    Transport {
        detail: String,
    },
    /// Not an empty observation: a blank one would read as a venue matching its profile.
    Unreadable {
        detail: String,
    },
    UnknownInstrument {
        instrument: String,
    },
}

impl core::fmt::Display for ObserveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport { detail } => write!(f, "the venue could not be reached: {detail}"),
            Self::Unreadable { detail } => {
                write!(f, "the venue's answer could not be read: {detail}")
            }
            Self::UnknownInstrument { instrument } => write!(f, "{instrument} is not listed"),
        }
    }
}

impl std::error::Error for ObserveError {}

/// Pure over the `meta` answer so it can be driven from a recording. What the venue states is
/// observed: the size decimals, from which the tick, the lot and the price rule follow, and the
/// coin, which is what a size counts. The collateral is not in the answer, so quote and
/// settlement are left unobserved rather than assumed.
pub fn read_instrument(body: &str, instrument: &str) -> Result<ObservedBehaviour, ObserveError> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ObserveError::Unreadable { detail: e.to_string() })?;
    let universe = parsed
        .get("universe")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ObserveError::Unreadable { detail: "no universe".into() })?;
    let row = universe
        .iter()
        .find(|r| r.get("name").and_then(serde_json::Value::as_str) == Some(instrument))
        .ok_or_else(|| ObserveError::UnknownInstrument { instrument: instrument.to_string() })?;
    let size_decimals = row
        .get("szDecimals")
        .and_then(serde_json::Value::as_u64)
        .and_then(|d| u8::try_from(d).ok())
        .filter(|d| *d <= PERP_MAX_DECIMALS)
        .ok_or_else(|| ObserveError::Unreadable { detail: "no usable szDecimals".into() })?;
    let symbol = VenueSymbol::new(instrument);
    Ok(ObservedBehaviour {
        precision: Some(ObservedPrecision {
            symbol: symbol.clone(),
            rules: PrecisionRules {
                price_scale: PERP_MAX_DECIMALS - size_decimals,
                quantity_scale: size_decimals,
                price_tick: 1,
                quantity_lot: 1,
                price_validity: PriceValidity::SignificantFigures { at_most: SIGNIFICANT_FIGURES },
            },
        }),
        currencies: Some(ObservedCurrencies {
            symbol,
            base: instrument.to_string(),
            quote: None,
            settlement: None,
        }),
        ..Default::default()
    })
}

/// Fetch, kept separate so venue assurance can digest the bytes the venue sent.
pub async fn fetch_instrument(endpoint: &str) -> Result<String, ObserveError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .use_rustls_tls()
        .build()
        .map_err(|e| ObserveError::Transport { detail: e.to_string() })?;
    let response = http
        .post(format!("{endpoint}{PATH_INFO}"))
        .header("Content-Type", "application/json")
        .body(META_BODY)
        .send()
        .await
        .map_err(|e| ObserveError::Transport { detail: e.to_string() })?;
    response.text().await.map_err(|e| ObserveError::Transport { detail: e.to_string() })
}
