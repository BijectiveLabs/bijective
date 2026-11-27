// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reads the venue's published instrument rules into an observation for venue assurance.
//! Design notes: `docs/integrations/bybit.md`.

use bijective_venue::normalise::{DecimalError, Scale, parse_scaled};
use bijective_venue::profile::{
    FundingModel, ObservedBehaviour, ObservedCurrencies, ObservedMinimums, ObservedPrecision,
    PrecisionRules, PriceValidity, Transport, VenueSymbol,
};

/// Public instrument endpoint. Needs no credential.
const PATH_INSTRUMENTS: &str = "/v5/market/instruments-info";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveError {
    Transport {
        detail: String,
    },
    /// Not an empty observation: a blank one would read as a venue matching its profile.
    Unreadable {
        detail: String,
    },
    /// The venue does not list the instrument.
    UnknownInstrument {
        instrument: String,
    },
    /// A published increment we could not read exactly.
    Increment {
        field: &'static str,
        cause: DecimalError,
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
            Self::Increment { field, cause } => {
                write!(f, "{field} could not be read exactly: {cause}")
            }
        }
    }
}

impl std::error::Error for ObserveError {}

/// Pure over the response body so it can be driven from a recording.
pub fn read_instrument(body: &str, instrument: &str) -> Result<ObservedBehaviour, ObserveError> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ObserveError::Unreadable { detail: e.to_string() })?;
    let row = instrument_row(&parsed, instrument)?;
    let (rules, price, quantity) = precision_of(row)?;
    let mut observed = ObservedBehaviour {
        precision: Some(ObservedPrecision { symbol: VenueSymbol::new(instrument), rules }),
        minimums: minimums_of(row, instrument, price, quantity)?,
        currencies: Some(currencies_of(row, instrument)?),
        ..Default::default()
    };

    // The venue publishes minutes. Absent rather than guessed (see FundingModel::Continuous).
    if let Some(minutes) = row.get("fundingInterval").and_then(serde_json::Value::as_i64) {
        observed.funding.insert(
            Transport::Stream,
            FundingModel::Published { interval_ns: minutes as i128 * 60_000_000_000 },
        );
    }
    Ok(observed)
}

/// The row for `instrument` in a successful instruments answer.
fn instrument_row<'a>(
    parsed: &'a serde_json::Value,
    instrument: &str,
) -> Result<&'a serde_json::Value, ObserveError> {
    if parsed.get("retCode").and_then(serde_json::Value::as_i64) != Some(0) {
        return Err(ObserveError::Unreadable {
            detail: parsed
                .get("retMsg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("the venue refused the read")
                .to_string(),
        });
    }
    let rows = parsed
        .pointer("/result/list")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ObserveError::Unreadable { detail: "no instrument list".into() })?;
    rows.iter()
        .find(|r| r.get("symbol").and_then(serde_json::Value::as_str) == Some(instrument))
        .ok_or_else(|| ObserveError::UnknownInstrument { instrument: instrument.to_string() })
}

/// Price and quantity scales, tick and lot, read from the venue's own steps rather than assumed.
fn precision_of(row: &serde_json::Value) -> Result<(PrecisionRules, Scale, Scale), ObserveError> {
    let price_scale = row
        .get("priceScale")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<u8>().ok())
        .or_else(|| row.get("priceScale").and_then(serde_json::Value::as_u64).map(|v| v as u8))
        .ok_or_else(|| ObserveError::Unreadable { detail: "no price scale".into() })?;
    let qty_step = row
        .pointer("/lotSizeFilter/qtyStep")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ObserveError::Unreadable { detail: "no quantity step".into() })?;
    let tick = row
        .pointer("/priceFilter/tickSize")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ObserveError::Unreadable { detail: "no tick size".into() })?;

    let quantity_scale = decimals_in(qty_step);
    let quantity = Scale::new(quantity_scale)
        .map_err(|e| ObserveError::Unreadable { detail: e.to_string() })?;
    let price =
        Scale::new(price_scale).map_err(|e| ObserveError::Unreadable { detail: e.to_string() })?;
    let quantity_lot = parse_scaled(qty_step, quantity)
        .map_err(|cause| ObserveError::Increment { field: "qtyStep", cause })?;
    let price_tick = parse_scaled(tick, price)
        .map_err(|cause| ObserveError::Increment { field: "tickSize", cause })?;
    let rules = PrecisionRules {
        price_scale,
        quantity_scale,
        price_tick,
        quantity_lot,
        // The venue publishes only a tick, so the increment is the whole rule.
        price_validity: PriceValidity::IncrementOnly,
    };
    Ok((rules, price, quantity))
}

/// Minimum order size and notional. A minimum equal to the step is common but a different fact,
/// so the floors are read from the row. Absent is not zero: with no published floor the verifier
/// reports nothing, not agreement.
fn minimums_of(
    row: &serde_json::Value,
    instrument: &str,
    price: Scale,
    quantity: Scale,
) -> Result<Option<ObservedMinimums>, ObserveError> {
    let size = row.pointer("/lotSizeFilter/minOrderQty").and_then(serde_json::Value::as_str);
    let notional =
        row.pointer("/lotSizeFilter/minNotionalValue").and_then(serde_json::Value::as_str);
    let (Some(size), Some(notional)) = (size, notional) else {
        return Ok(None);
    };
    Ok(Some(ObservedMinimums {
        symbol: VenueSymbol::new(instrument),
        minimum_size: parse_scaled(size, quantity)
            .map_err(|cause| ObserveError::Increment { field: "minOrderQty", cause })?,
        minimum_notional: parse_scaled(notional, price)
            .map_err(|cause| ObserveError::Increment { field: "minNotionalValue", cause })?,
    }))
}

/// `baseCoin`, `quoteCoin` and `settleCoin` (Instruments info, read 2026-11-15). Each is required:
/// a linear perpetual the venue lists without one is a row this build cannot vouch for.
fn currencies_of(
    row: &serde_json::Value,
    instrument: &str,
) -> Result<ObservedCurrencies, ObserveError> {
    let coin = |field: &str| {
        row.get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ObserveError::Unreadable { detail: format!("no {field}") })
    };
    Ok(ObservedCurrencies {
        symbol: VenueSymbol::new(instrument),
        base: coin("baseCoin")?,
        quote: Some(coin("quoteCoin")?),
        settlement: Some(coin("settleCoin")?),
    })
}

/// Fetch and read. Public, so it needs no credential and can run in a gate.
pub async fn observe_instrument(
    endpoint: &str,
    instrument: &str,
) -> Result<ObservedBehaviour, ObserveError> {
    read_instrument(&fetch_instrument(endpoint, instrument).await?, instrument)
}

/// Kept separate so venue assurance can digest the bytes the venue sent.
pub async fn fetch_instrument(endpoint: &str, instrument: &str) -> Result<String, ObserveError> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let url = format!("{endpoint}{PATH_INSTRUMENTS}?category=linear&symbol={instrument}");
    let http = reqwest::Client::builder()
        .timeout(crate::rest::DEFAULT_TIMEOUT)
        .use_rustls_tls()
        .build()
        .map_err(|e| ObserveError::Transport { detail: e.to_string() })?;

    let response = http
        .get(url)
        .send()
        .await
        .map_err(|e| ObserveError::Transport { detail: e.to_string() })?;
    response.text().await.map_err(|e| ObserveError::Transport { detail: e.to_string() })
}

fn decimals_in(decimal: &str) -> u8 {
    decimal.split_once('.').map_or(0, |(_, fraction)| fraction.trim_end().len() as u8)
}
