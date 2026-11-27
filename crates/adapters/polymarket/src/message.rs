// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The market stream's frames, read into typed events at the platform's scales.
//! Sources and what was measured on the live stream: `docs/integrations/polymarket.md`.

use bijective_venue::contract::market::BookSide;
use bijective_venue::normalise::{Scale, parse_scaled};

/// The venue asks for a `PING` text frame every ten seconds (Real-time data).
pub const PING_INTERVAL_NS: i128 = 10_000_000_000;

/// One price level, at the platform's scales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// A whole book for one outcome. Sides are best first whatever order the venue sent them in:
/// the live stream sends both ascending from the far end (measured 2026-11-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireBook {
    pub asset: String,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
    pub ts_ms: u64,
}

/// One changed level, and the best prices the venue states after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireChange {
    pub asset: String,
    pub side: BookSide,
    pub price_scaled: i128,
    /// Zero removes the level.
    pub quantity_scaled: i128,
    /// `None` where the venue left it out. A side with no level is stated as `0`, which no level
    /// can be priced at (measured 2026-11-15).
    pub best_bid_scaled: Option<i128>,
    pub best_ask_scaled: Option<i128>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireTrade {
    pub asset: String,
    pub price_scaled: i128,
    pub quantity_scaled: i128,
    /// The settling transaction. Several trades of one asset can share it.
    pub transaction: String,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueEvent {
    Book(WireBook),
    Changes {
        changes: Vec<WireChange>,
        ts_ms: u64,
    },
    Trade(WireTrade),
    /// The tick changed: an order off the new tick is rejected (Place orders).
    TickSize {
        asset: String,
        tick_scaled: i128,
        ts_ms: u64,
    },
    /// The question resolved and this outcome won.
    Resolved {
        assets: Vec<String>,
        winning: String,
        ts_ms: u64,
    },
    Pong,
    /// Understood and carrying nothing the platform records: `best_bid_ask`, `new_market`, an
    /// empty acknowledgement, `NO NEW ASSETS`.
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotJson(String),
    /// A field missing or not in the form the documents state, named.
    Field {
        event: String,
        field: &'static str,
    },
    /// An event type this build does not know. Carried, not dropped, so a format change is seen.
    UnknownEvent(String),
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotJson(detail) => write!(f, "not JSON: {detail}"),
            Self::Field { event, field } => write!(f, "{event}: {field} is missing or malformed"),
            Self::UnknownEvent(kind) => {
                write!(f, "an event of type {kind} this build does not know")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// One frame. The opening snapshot arrives as an array of `book` events. Every later event as
/// one object (measured 2026-11-15).
pub fn parse(frame: &[u8], price: Scale, quantity: Scale) -> Result<Vec<VenueEvent>, ParseError> {
    if frame == b"PONG" {
        return Ok(vec![VenueEvent::Pong]);
    }
    // The answer to subscribing an outcome already subscribed: no snapshot follows (measured
    // 2026-11-15).
    if frame == b"NO NEW ASSETS" {
        return Ok(vec![VenueEvent::Ignored]);
    }
    let value: serde_json::Value =
        serde_json::from_slice(frame).map_err(|e| ParseError::NotJson(e.to_string()))?;
    let scales = Scales { price, quantity };
    match value {
        serde_json::Value::Array(events) if events.is_empty() => Ok(vec![VenueEvent::Ignored]),
        serde_json::Value::Array(events) => events.iter().map(|e| event(e, scales)).collect(),
        single => Ok(vec![event(&single, scales)?]),
    }
}

#[derive(Clone, Copy)]
struct Scales {
    price: Scale,
    quantity: Scale,
}

fn event(value: &serde_json::Value, scales: Scales) -> Result<VenueEvent, ParseError> {
    let kind = text(value, "?", "event_type")?;
    let field = |name: &'static str| text(value, kind, name);
    match kind {
        "book" => Ok(VenueEvent::Book(WireBook {
            asset: field("asset_id")?.to_string(),
            bids: levels(value, kind, "bids", scales, true)?,
            asks: levels(value, kind, "asks", scales, false)?,
            ts_ms: millis(field("timestamp")?, kind)?,
        })),
        "price_change" => changes(value, scales),
        "last_trade_price" => Ok(VenueEvent::Trade(WireTrade {
            asset: field("asset_id")?.to_string(),
            price_scaled: scaled(field("price")?, scales.price, kind, "price")?,
            quantity_scaled: scaled(field("size")?, scales.quantity, kind, "size")?,
            transaction: field("transaction_hash")?.to_string(),
            ts_ms: millis(field("timestamp")?, kind)?,
        })),
        "tick_size_change" => Ok(VenueEvent::TickSize {
            asset: field("asset_id")?.to_string(),
            tick_scaled: scaled(field("new_tick_size")?, scales.price, kind, "new_tick_size")?,
            ts_ms: millis(field("timestamp")?, kind)?,
        }),
        "market_resolved" => Ok(VenueEvent::Resolved {
            assets: strings(value, kind, "assets_ids")?,
            winning: field("winning_asset_id")?.to_string(),
            ts_ms: millis(field("timestamp")?, kind)?,
        }),
        "best_bid_ask" | "new_market" => Ok(VenueEvent::Ignored),
        other => Err(ParseError::UnknownEvent(other.to_string())),
    }
}

fn changes(value: &serde_json::Value, scales: Scales) -> Result<VenueEvent, ParseError> {
    let kind = "price_change";
    let missing = |field| ParseError::Field { event: kind.into(), field };
    let rows = value
        .get("price_changes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| missing("price_changes"))?;
    let mut changes = Vec::with_capacity(rows.len());
    for row in rows {
        let side = match text(row, kind, "side")? {
            "BUY" => BookSide::Bid,
            "SELL" => BookSide::Ask,
            _ => return Err(missing("side")),
        };
        let best = |name: &'static str| -> Result<Option<i128>, ParseError> {
            match row.get(name).and_then(serde_json::Value::as_str) {
                None | Some("") => Ok(None),
                Some(t) => scaled(t, scales.price, kind, name).map(Some),
            }
        };
        changes.push(WireChange {
            asset: text(row, kind, "asset_id")?.to_string(),
            side,
            price_scaled: scaled(text(row, kind, "price")?, scales.price, kind, "price")?,
            quantity_scaled: scaled(text(row, kind, "size")?, scales.quantity, kind, "size")?,
            best_bid_scaled: best("best_bid")?,
            best_ask_scaled: best("best_ask")?,
        });
    }
    Ok(VenueEvent::Changes { changes, ts_ms: millis(text(value, kind, "timestamp")?, kind)? })
}

/// Best first: bids descending, asks ascending.
fn levels(
    value: &serde_json::Value,
    kind: &str,
    side: &'static str,
    scales: Scales,
    descending: bool,
) -> Result<Vec<Level>, ParseError> {
    let rows = value
        .get(side)
        .and_then(serde_json::Value::as_array)
        .ok_or(ParseError::Field { event: kind.into(), field: side })?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(Level {
            price_scaled: scaled(text(row, kind, "price")?, scales.price, kind, "price")?,
            quantity_scaled: scaled(text(row, kind, "size")?, scales.quantity, kind, "size")?,
        });
    }
    out.sort_by_key(|l| l.price_scaled);
    if descending {
        out.reverse();
    }
    Ok(out)
}

fn text<'v>(
    value: &'v serde_json::Value,
    kind: &str,
    field: &'static str,
) -> Result<&'v str, ParseError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or(ParseError::Field { event: kind.into(), field })
}

fn strings(
    value: &serde_json::Value,
    kind: &str,
    field: &'static str,
) -> Result<Vec<String>, ParseError> {
    let missing = || ParseError::Field { event: kind.into(), field };
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(missing)?
        .iter()
        .map(|v| v.as_str().map(str::to_string).ok_or_else(missing))
        .collect()
}

/// A figure finer than the scale is refused, never rounded.
fn scaled(text: &str, scale: Scale, kind: &str, field: &'static str) -> Result<i128, ParseError> {
    parse_scaled(text, scale).map_err(|_| ParseError::Field { event: kind.into(), field })
}

fn millis(text: &str, kind: &str) -> Result<u64, ParseError> {
    text.parse::<u64>().map_err(|_| ParseError::Field { event: kind.into(), field: "timestamp" })
}
