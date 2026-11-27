// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Parsing what the venue sends, refusing what it cannot read.

use bijective_model::VenueSymbol;
use bijective_model::market::ScaledDecimal;
use bijective_venue::normalise::{DecimalError, Scale, parse_scaled};
use bijective_venue::profile::{InstrumentScales, Scales};
use serde::Deserialize;

use crate::book::Level;

/// Venue limit on topics per connection. Belongs in the profile once it has a connection section.
pub const MAX_TOPICS_PER_CONNECTION: usize = 10;

/// The venue drops a connection unpinged for 30s. 20s keeps margin against late packets.
pub const PING_INTERVAL_NS: i128 = 20_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookUpdateKind {
    Snapshot,
    Delta,
}

/// A message, once understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueMessage {
    Book {
        symbol: String,
        kind: BookUpdateKind,
        bids: Vec<Level>,
        asks: Vec<Level>,
        seq: u64,
        ts_ms: u64,
    },
    Trades {
        symbol: String,
        trades: Vec<WireTrade>,
    },
    Liquidations {
        symbol: String,
        liquidations: Vec<WireLiquidation>,
    },
    /// Snapshot and delta alike: a field that is absent has not changed, so absence is kept.
    Ticker {
        symbol: String,
        ts_ms: u64,
        fields: Box<TickerFields>,
    },
    /// The venue's answer to a subscribe request - the confirmed-subscription rule's confirmation, or its refusal.
    SubscribeResult {
        op: String,
        success: bool,
    },
    Pong,
    /// Valid JSON we do not handle. Named so the verifier sees new venue message classes.
    Unrecognised {
        topic: Option<String>,
    },
}

/// One execution. `taker_buys` is the venue's taker side: the aggressor, not the resting order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireTrade {
    pub ts_ms: u64,
    pub taker_buys: bool,
    pub id: String,
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// One forced closure. The venue states the side of the liquidated POSITION: a long is `Buy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireLiquidation {
    pub ts_ms: u64,
    pub position_was_long: bool,
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// What a ticker message carried. The funding rate is kept as written, with the scale its digits
/// imply, because the venue's unit is not converted here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickerFields {
    pub funding_rate: Option<ScaledDecimal>,
    pub next_funding_ms: Option<u64>,
    pub open_interest_scaled: Option<i128>,
    pub mark_price_scaled: Option<i128>,
    pub index_price_scaled: Option<i128>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotJson,
    /// A book message missing something a book message must have.
    MissingField {
        name: &'static str,
    },
    UnknownBookType {
        found: String,
    },
    /// Refused, never dropped: skipping an unreadable level leaves the book wrong.
    Level {
        side: &'static str,
        index: usize,
        source: DecimalError,
    },
    TopicMalformed {
        topic: String,
    },
    /// A field of a trade, liquidation or ticker message that is missing or unreadable.
    Field {
        topic: String,
        name: &'static str,
    },
    /// A topic of an instrument the profile does not list: no scale to read it at.
    UnlistedSymbol {
        topic: String,
    },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::NotJson => write!(f, "message is not JSON"),
            ParseError::MissingField { name } => write!(f, "book message has no {name}"),
            ParseError::UnknownBookType { found } => write!(f, "unknown book message type {found}"),
            ParseError::Level { side, index, source } => {
                write!(f, "{side} level {index} could not be read: {source}")
            }
            ParseError::TopicMalformed { topic } => write!(f, "topic {topic} is not a book topic"),
            ParseError::Field { topic, name } => {
                write!(f, "{topic}: {name} is missing or unreadable")
            }
            ParseError::UnlistedSymbol { topic } => {
                write!(f, "{topic} names an instrument the profile does not list")
            }
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Deserialize)]
struct Envelope {
    topic: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    ts: Option<u64>,
    data: Option<serde_json::Value>,
    op: Option<String>,
    success: Option<bool>,
    ret_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BookData {
    s: String,
    #[serde(default)]
    b: Vec<[String; 2]>,
    #[serde(default)]
    a: Vec<[String; 2]>,
    seq: u64,
}

/// Scales come from the venue profile, per instrument, chosen by the symbol that ends the topic.
/// Values arrive as decimal strings and convert exactly.
pub fn parse(text: &str, scales: &InstrumentScales) -> Result<VenueMessage, ParseError> {
    let env: Envelope = serde_json::from_str(text).map_err(|_| ParseError::NotJson)?;

    if let Some(op) = env.op {
        if op == "pong" || env.ret_msg.as_deref() == Some("pong") {
            return Ok(VenueMessage::Pong);
        }
        return Ok(VenueMessage::SubscribeResult { success: env.success.unwrap_or(false), op });
    }

    let Some(topic) = env.topic else {
        return Ok(VenueMessage::Unrecognised { topic: None });
    };
    const READ: [&str; 4] = ["publicTrade.", "allLiquidation.", "tickers.", "orderbook."];
    if !READ.iter().any(|prefix| topic.starts_with(prefix)) {
        return Ok(VenueMessage::Unrecognised { topic: Some(topic) });
    }
    let Scales { price, quantity } = scales_for(&topic, scales)?;
    if topic.starts_with("publicTrade.") {
        return parse_trades(topic, env.data, price, quantity);
    }
    if topic.starts_with("allLiquidation.") {
        return parse_liquidations(topic, env.data, price, quantity);
    }
    if topic.starts_with("tickers.") {
        return parse_ticker(topic, env.data, env.ts, price, quantity);
    }

    let kind = match env.kind.as_deref() {
        Some("snapshot") => BookUpdateKind::Snapshot,
        Some("delta") => BookUpdateKind::Delta,
        Some(other) => {
            return Err(ParseError::UnknownBookType { found: other.to_string() });
        }
        None => return Err(ParseError::MissingField { name: "type" }),
    };

    let raw = env.data.ok_or(ParseError::MissingField { name: "data" })?;
    let data: BookData =
        serde_json::from_value(raw).map_err(|_| ParseError::MissingField { name: "data" })?;
    if data.s.is_empty() {
        return Err(ParseError::TopicMalformed { topic });
    }

    Ok(VenueMessage::Book {
        symbol: data.s,
        kind,
        bids: levels(&data.b, "bid", price, quantity)?,
        asks: levels(&data.a, "ask", price, quantity)?,
        seq: data.seq,
        ts_ms: env.ts.ok_or(ParseError::MissingField { name: "ts" })?,
    })
}

#[derive(Debug, Deserialize)]
struct TradeData {
    #[serde(rename = "T")]
    ts_ms: u64,
    s: String,
    #[serde(rename = "S")]
    side: String,
    v: String,
    p: String,
    i: String,
}

#[derive(Debug, Deserialize)]
struct LiquidationData {
    #[serde(rename = "T")]
    ts_ms: u64,
    s: String,
    #[serde(rename = "S")]
    side: String,
    v: String,
    p: String,
}

#[derive(Debug, Deserialize)]
struct TickerData {
    symbol: String,
    #[serde(rename = "fundingRate")]
    funding_rate: Option<String>,
    #[serde(rename = "nextFundingTime")]
    next_funding_time: Option<String>,
    #[serde(rename = "openInterest")]
    open_interest: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(rename = "indexPrice")]
    index_price: Option<String>,
}

fn field(topic: &str, name: &'static str) -> ParseError {
    ParseError::Field { topic: topic.to_string(), name }
}

/// A side that is neither word is refused: guessing a side would record a fact the venue did not state.
fn is_buy(topic: &str, side: &str) -> Result<bool, ParseError> {
    match side {
        "Buy" => Ok(true),
        "Sell" => Ok(false),
        _ => Err(field(topic, "side")),
    }
}

/// The symbol is the topic's last segment (`orderbook.50.BTCUSDT`, `tickers.BTCUSDT`).
fn scales_for(topic: &str, scales: &InstrumentScales) -> Result<Scales, ParseError> {
    let symbol = topic.rsplit('.').next().unwrap_or_default();
    scales
        .of(&VenueSymbol::new(symbol))
        .ok_or_else(|| ParseError::UnlistedSymbol { topic: topic.to_string() })
}

fn parse_trades(
    topic: String,
    data: Option<serde_json::Value>,
    price: Scale,
    quantity: Scale,
) -> Result<VenueMessage, ParseError> {
    let rows: Vec<TradeData> = serde_json::from_value(data.ok_or_else(|| field(&topic, "data"))?)
        .map_err(|_| field(&topic, "data"))?;
    let mut symbol = String::new();
    let mut trades = Vec::with_capacity(rows.len());
    for row in rows {
        trades.push(WireTrade {
            ts_ms: row.ts_ms,
            taker_buys: is_buy(&topic, &row.side)?,
            id: row.i,
            price_scaled: parse_scaled(&row.p, price).map_err(|_| field(&topic, "p"))?,
            quantity_scaled: parse_scaled(&row.v, quantity).map_err(|_| field(&topic, "v"))?,
        });
        symbol = row.s;
    }
    Ok(VenueMessage::Trades { symbol, trades })
}

fn parse_liquidations(
    topic: String,
    data: Option<serde_json::Value>,
    price: Scale,
    quantity: Scale,
) -> Result<VenueMessage, ParseError> {
    let rows: Vec<LiquidationData> =
        serde_json::from_value(data.ok_or_else(|| field(&topic, "data"))?)
            .map_err(|_| field(&topic, "data"))?;
    let mut symbol = String::new();
    let mut liquidations = Vec::with_capacity(rows.len());
    for row in rows {
        liquidations.push(WireLiquidation {
            ts_ms: row.ts_ms,
            position_was_long: is_buy(&topic, &row.side)?,
            price_scaled: parse_scaled(&row.p, price).map_err(|_| field(&topic, "p"))?,
            quantity_scaled: parse_scaled(&row.v, quantity).map_err(|_| field(&topic, "v"))?,
        });
        symbol = row.s;
    }
    Ok(VenueMessage::Liquidations { symbol, liquidations })
}

fn parse_ticker(
    topic: String,
    data: Option<serde_json::Value>,
    ts: Option<u64>,
    price: Scale,
    quantity: Scale,
) -> Result<VenueMessage, ParseError> {
    let row: TickerData = serde_json::from_value(data.ok_or_else(|| field(&topic, "data"))?)
        .map_err(|_| field(&topic, "data"))?;
    let scaled = |text: &Option<String>, scale: Scale, name: &'static str| {
        text.as_deref().map(|t| parse_scaled(t, scale).map_err(|_| field(&topic, name))).transpose()
    };
    let fields = TickerFields {
        funding_rate: row
            .funding_rate
            .as_deref()
            .map(|t| decimal_as_written(t).map_err(|_| field(&topic, "fundingRate")))
            .transpose()?,
        next_funding_ms: row
            .next_funding_time
            .as_deref()
            .map(|t| t.parse::<u64>().map_err(|_| field(&topic, "nextFundingTime")))
            .transpose()?,
        open_interest_scaled: scaled(&row.open_interest, quantity, "openInterest")?,
        mark_price_scaled: scaled(&row.mark_price, price, "markPrice")?,
        index_price_scaled: scaled(&row.index_price, price, "indexPrice")?,
    };
    let ts_ms = ts.ok_or_else(|| field(&topic, "ts"))?;
    Ok(VenueMessage::Ticker { symbol: row.symbol, ts_ms, fields: Box::new(fields) })
}

/// A decimal with exactly the scale its digits show: "0.0001" is 1 at scale 4. Used where the
/// platform declares no scale for the figure, so nothing is rounded and nothing is assumed.
fn decimal_as_written(text: &str) -> Result<ScaledDecimal, DecimalError> {
    let digits = text.trim().split_once('.').map_or(0, |(_, fraction)| fraction.len());
    let scale = u8::try_from(digits).map_err(|_| DecimalError::Empty)?;
    let value_scaled = parse_scaled(text, Scale::new(scale).map_err(|_| DecimalError::Empty)?)?;
    Ok(ScaledDecimal { value_scaled, scale })
}

fn levels(
    raw: &[[String; 2]],
    side: &'static str,
    price: Scale,
    quantity: Scale,
) -> Result<Vec<Level>, ParseError> {
    let mut out = Vec::with_capacity(raw.len());
    for (index, [p, q]) in raw.iter().enumerate() {
        out.push(Level {
            price_scaled: parse_scaled(p, price).map_err(|source| ParseError::Level {
                side,
                index,
                source,
            })?,
            quantity_scaled: parse_scaled(q, quantity).map_err(|source| ParseError::Level {
                side,
                index,
                source,
            })?,
        });
    }
    Ok(out)
}
