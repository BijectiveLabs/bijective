// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The stock stream's frames read into messages (Real-time stock data, Streaming market data,
//! read 2026-11-17, `docs/integrations/alpaca.md`). Every frame is a JSON list of objects, each
//! typed by `T`. Prices are JSON numbers: each is read from its shortest decimal text, which is
//! the venue's own while a price has at most fifteen significant digits, and refused when finer
//! than the platform's scale.

use bijective_venue::normalise::{Scale, parse_scaled};
use serde_json::Value;

use crate::evidence::utc_ns;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub detail: String,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for ParseError {}

fn refused(detail: impl Into<String>) -> ParseError {
    ParseError { detail: detail.into() }
}

/// One side of a quote. A price or size of zero states an empty side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side {
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketMessage {
    Welcome,
    Authenticated,
    Subscribed,
    /// The venue's error table: 402 auth failed, 406 connection limit, 409 insufficient
    /// subscription, and others. The stream is not usable after it.
    Error {
        code: i64,
        message: String,
    },
    Quote {
        symbol: String,
        bid: Side,
        ask: Side,
        at_ns: Option<i128>,
    },
    Trade {
        symbol: String,
        trade_id: String,
        price_scaled: i128,
        quantity_scaled: i128,
        at_ns: Option<i128>,
    },
    /// A type this build does not read, carried so it is seen rather than dropped.
    Other {
        kind: String,
    },
}

fn text<'a>(row: &'a Value, field: &str) -> Result<&'a str, ParseError> {
    row.get(field).and_then(Value::as_str).ok_or_else(|| refused(format!("{field} absent")))
}

/// A JSON number or string as decimal text, then scaled exactly.
fn scaled(row: &Value, field: &str, scale: Scale) -> Result<i128, ParseError> {
    let value = row.get(field).ok_or_else(|| refused(format!("{field} absent")))?;
    let decimal = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        _ => return Err(refused(format!("{field} is not a number"))),
    };
    parse_scaled(&decimal, scale).map_err(|e| refused(format!("{field} {decimal}: {e:?}")))
}

fn side(row: &Value, price: &str, size: &str, scales: (Scale, Scale)) -> Result<Side, ParseError> {
    Ok(Side {
        price_scaled: scaled(row, price, scales.0)?,
        quantity_scaled: scaled(row, size, scales.1)?,
    })
}

fn message(row: &Value, scales: (Scale, Scale)) -> Result<MarketMessage, ParseError> {
    let at_ns = || row.get("t").and_then(Value::as_str).and_then(utc_ns);
    Ok(match text(row, "T")? {
        "success" => match text(row, "msg")? {
            "connected" => MarketMessage::Welcome,
            "authenticated" => MarketMessage::Authenticated,
            other => MarketMessage::Other { kind: format!("success {other}") },
        },
        "error" => MarketMessage::Error {
            code: row.get("code").and_then(Value::as_i64).unwrap_or_default(),
            message: row.get("msg").and_then(Value::as_str).unwrap_or_default().to_string(),
        },
        "subscription" => MarketMessage::Subscribed,
        "q" => MarketMessage::Quote {
            symbol: text(row, "S")?.to_string(),
            bid: side(row, "bp", "bs", scales)?,
            ask: side(row, "ap", "as", scales)?,
            at_ns: at_ns(),
        },
        "t" => MarketMessage::Trade {
            symbol: text(row, "S")?.to_string(),
            trade_id: match row.get("i") {
                Some(Value::Number(id)) => id.to_string(),
                Some(Value::String(id)) => id.clone(),
                _ => return Err(refused("i absent")),
            },
            price_scaled: scaled(row, "p", scales.0)?,
            quantity_scaled: scaled(row, "s", scales.1)?,
            at_ns: at_ns(),
        },
        other => MarketMessage::Other { kind: other.to_string() },
    })
}

/// Every message of one frame, or the first that could not be read.
pub fn parse(
    frame: &[u8],
    price: Scale,
    quantity: Scale,
) -> Result<Vec<MarketMessage>, ParseError> {
    let parsed: Value = serde_json::from_slice(frame).map_err(|_| refused("not JSON"))?;
    let Value::Array(rows) = parsed else { return Err(refused("not a list")) };
    rows.iter().map(|row| message(row, (price, quantity))).collect()
}

/// The authentication message (Streaming market data). It carries the secret key. It is built
/// once and never logged.
#[must_use]
pub fn auth_frame(key_id: &str, secret: &str) -> String {
    serde_json::json!({ "action": "auth", "key": key_id, "secret": secret }).to_string()
}

#[must_use]
pub fn subscribe_frame(symbols: &[String]) -> String {
    serde_json::json!({ "action": "subscribe", "quotes": symbols, "trades": symbols }).to_string()
}
