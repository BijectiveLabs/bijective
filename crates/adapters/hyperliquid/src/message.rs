// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Parses venue frames. Venue sources in `docs/integrations/hyperliquid.md`.

use bijective_venue::contract::market::ScaledDecimal;
use bijective_venue::normalise::{DecimalError, Scale, parse_scaled};
use serde::Deserialize;

/// The venue drops idle connections after sixty seconds. Ping at half that.
pub const PING_INTERVAL_NS: i128 = 30_000_000_000;

/// One price level, as the venue writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// A complete book replaces both sides. With no sequence number a dropped payload is
/// undetectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueMessage {
    Book {
        symbol: String,
        bids: Vec<Level>,
        asks: Vec<Level>,
        venue_ts_ms: u64,
    },
    Trades {
        trades: Vec<WireTrade>,
    },
    /// The whole context, restated about once a second. It carries no time of its own.
    AssetContext {
        symbol: String,
        context: Box<AssetContext>,
    },
    Pong,
    /// Carries no identity of what it acknowledged.
    SubscriptionAccepted,
    /// Free text with no reference to its cause. Reported, not matched.
    Refused {
        detail: String,
    },
    Ignored,
}

/// One execution. `taker_buys` reads the venue's side letter as the taker's side. The venue's page
/// does not define the letter. A measurement against the book supports it (see the feed tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireTrade {
    pub symbol: String,
    pub ts_ms: u64,
    pub taker_buys: bool,
    pub id: String,
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// What a perpetual's context states. The funding rate is the venue's figure as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetContext {
    pub funding_rate: ScaledDecimal,
    pub open_interest_scaled: i128,
    pub mark_price_scaled: i128,
    pub oracle_price_scaled: i128,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    channel: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct BookData {
    coin: String,
    time: u64,
    levels: Vec<Vec<WireLevel>>,
}

#[derive(Debug, Deserialize)]
struct TradeData {
    coin: String,
    side: String,
    px: String,
    sz: String,
    time: u64,
    tid: u64,
}

#[derive(Debug, Deserialize)]
struct ContextData {
    coin: String,
    ctx: ContextFields,
}

/// Strings on the wire. The venue's page types them as numbers.
#[derive(Debug, Deserialize)]
struct ContextFields {
    funding: String,
    #[serde(rename = "openInterest")]
    open_interest: String,
    #[serde(rename = "oraclePx")]
    oracle_px: String,
    #[serde(rename = "markPx")]
    mark_px: String,
}

/// Decimal strings, parsed without floats.
#[derive(Debug, Deserialize)]
struct WireLevel {
    px: String,
    sz: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotJson,
    Malformed {
        field: &'static str,
    },
    Figure {
        field: &'static str,
        cause: DecimalError,
    },
    /// Bids then asks.
    Sides {
        found: usize,
    },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotJson => write!(f, "the frame is not json"),
            Self::Malformed { field } => write!(f, "{field} is missing or the wrong shape"),
            Self::Figure { field, cause } => write!(f, "{field} could not be read: {cause:?}"),
            Self::Sides { found } => write!(f, "a book with {found} sides, not two"),
        }
    }
}

pub fn parse(frame: &[u8], price: Scale, quantity: Scale) -> Result<VenueMessage, ParseError> {
    let envelope: Envelope = serde_json::from_slice(frame).map_err(|_| ParseError::NotJson)?;

    match envelope.channel.as_str() {
        "l2Book" => book(envelope.data, price, quantity),
        "trades" => trades(envelope.data, price, quantity),
        "activeAssetCtx" => asset_context(envelope.data, price, quantity),
        "pong" => Ok(VenueMessage::Pong),
        "subscriptionResponse" => Ok(VenueMessage::SubscriptionAccepted),
        "error" => {
            Ok(VenueMessage::Refused { detail: envelope.data.as_str().unwrap_or("").to_string() })
        }
        _ => Ok(VenueMessage::Ignored),
    }
}

fn book(
    data: serde_json::Value,
    price: Scale,
    quantity: Scale,
) -> Result<VenueMessage, ParseError> {
    let data: BookData =
        serde_json::from_value(data).map_err(|_| ParseError::Malformed { field: "data" })?;

    // Any other side count is refused: a book missing its asks is not a book with no asks.
    if data.levels.len() != 2 {
        return Err(ParseError::Sides { found: data.levels.len() });
    }

    let mut sides = data.levels.into_iter();
    let bids = side(sides.next().unwrap_or_default(), price, quantity, "bids")?;
    let asks = side(sides.next().unwrap_or_default(), price, quantity, "asks")?;

    Ok(VenueMessage::Book { symbol: data.coin, bids, asks, venue_ts_ms: data.time })
}

fn trades(
    data: serde_json::Value,
    price: Scale,
    quantity: Scale,
) -> Result<VenueMessage, ParseError> {
    let rows: Vec<TradeData> =
        serde_json::from_value(data).map_err(|_| ParseError::Malformed { field: "data" })?;
    let figure = |text: &str, scale, field| {
        parse_scaled(text, scale).map_err(|cause| ParseError::Figure { field, cause })
    };
    let mut trades = Vec::with_capacity(rows.len());
    for row in rows {
        // A letter that is neither is refused: guessing a side would record a fact not stated.
        let taker_buys = match row.side.as_str() {
            "B" => true,
            "A" => false,
            _ => return Err(ParseError::Malformed { field: "side" }),
        };
        trades.push(WireTrade {
            price_scaled: figure(&row.px, price, "px")?,
            quantity_scaled: figure(&row.sz, quantity, "sz")?,
            symbol: row.coin,
            ts_ms: row.time,
            taker_buys,
            id: row.tid.to_string(),
        });
    }
    Ok(VenueMessage::Trades { trades })
}

fn asset_context(
    data: serde_json::Value,
    price: Scale,
    quantity: Scale,
) -> Result<VenueMessage, ParseError> {
    let data: ContextData =
        serde_json::from_value(data).map_err(|_| ParseError::Malformed { field: "data" })?;
    let figure = |text: &str, scale, field| {
        parse_scaled(text, scale).map_err(|cause| ParseError::Figure { field, cause })
    };
    let context = AssetContext {
        funding_rate: decimal_as_written(&data.ctx.funding)
            .map_err(|cause| ParseError::Figure { field: "funding", cause })?,
        open_interest_scaled: figure(&data.ctx.open_interest, quantity, "openInterest")?,
        mark_price_scaled: figure(&data.ctx.mark_px, price, "markPx")?,
        oracle_price_scaled: figure(&data.ctx.oracle_px, price, "oraclePx")?,
    };
    Ok(VenueMessage::AssetContext { symbol: data.coin, context: Box::new(context) })
}

/// A decimal with exactly the scale its digits show: "0.0000125" is 125 at scale 7. Used where the
/// platform declares no scale for the figure, so nothing is rounded and nothing is assumed.
fn decimal_as_written(text: &str) -> Result<ScaledDecimal, DecimalError> {
    let digits = text.trim().split_once('.').map_or(0, |(_, fraction)| fraction.len());
    let scale = u8::try_from(digits).map_err(|_| DecimalError::Empty)?;
    let value_scaled = parse_scaled(text, Scale::new(scale).map_err(|_| DecimalError::Empty)?)?;
    Ok(ScaledDecimal { value_scaled, scale })
}

fn side(
    raw: Vec<WireLevel>,
    price: Scale,
    quantity: Scale,
    field: &'static str,
) -> Result<Vec<Level>, ParseError> {
    raw.into_iter()
        .map(|l| {
            Ok(Level {
                price_scaled: parse_scaled(&l.px, price)
                    .map_err(|cause| ParseError::Figure { field, cause })?,
                quantity_scaled: parse_scaled(&l.sz, quantity)
                    .map_err(|cause| ParseError::Figure { field, cause })?,
            })
        })
        .collect()
}
