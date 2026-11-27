// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The market facts a strategy may read besides the top of book: trades, liquidations, funding,
//! open interest, mark and index prices, long/short ratios and resolutions, one stream per venue.
//!
//! A fact keeps both of its instants and is named by the platform's instrument identity. Full
//! book snapshots and deltas are not facts here: their top is on the market stream, and the full
//! depth is in the archive.

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_model::market::{
    Aggressor, ExchangeTime, Funding, Liquidation, LongShortRatio, MarketFact, OpenInterest,
    ScaledDecimal, Trade,
};
use bijective_model::{InstrumentId, Side};

pub const VERSION: u16 = 1;

const PREFIX: &str = "facts-";

/// The stream one venue's facts are written to. One per venue, because each venue's data engine
/// writes its own and a stream has one writer.
#[must_use]
pub fn stream(venue: &str) -> String {
    format!("{PREFIX}{venue}")
}

/// The venue a facts stream belongs to, or nothing where the name is not one.
#[must_use]
pub fn venue_of(stream: &str) -> Option<&str> {
    stream.strip_prefix(PREFIX).filter(|rest| !rest.is_empty())
}

/// One fact about one instrument, with both of its instants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactUpdate {
    /// The platform's identity, resolved by the data engine through the declared catalogue.
    pub instrument: InstrumentId,
    pub exchange_time: ExchangeTime,
    /// This node's clock when the frame carrying the fact was read from the socket.
    pub received_at_ns: i128,
    pub fact: MarketFact,
}

#[derive(Debug)]
pub enum FactError {
    Encode(EncodeError),
    /// A book snapshot or delta: its top travels on the market stream, not here.
    NotAFact,
}

const TRADE: u8 = 1;
const LIQUIDATION: u8 = 2;
const FUNDING: u8 = 3;
const OPEN_INTEREST_BASE: u8 = 4;
const OPEN_INTEREST_QUOTE: u8 = 5;
const MARK_PRICE: u8 = 6;
const INDEX_PRICE: u8 = 7;
const LONG_SHORT_RATIO: u8 = 8;
const RESOLUTION: u8 = 9;

/// Whether `fact` is carried on a facts stream.
#[must_use]
pub fn is_fact(fact: &MarketFact) -> bool {
    !matches!(fact, MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_))
}

pub fn encode(update: &FactUpdate) -> Result<Vec<u8>, FactError> {
    let mut w = Writer::new(ClassId::MarketFact, VERSION);
    w.str(update.instrument.as_str()).map_err(FactError::Encode)?;
    time(&mut w, update.exchange_time);
    w.i128(update.received_at_ns);
    match &update.fact {
        MarketFact::Trade(t) => {
            w.u8(TRADE).str(&t.trade_id).map_err(FactError::Encode)?;
            w.u8(aggressor_code(t.aggressor)).i128(t.price_scaled).i128(t.quantity_scaled);
        }
        MarketFact::Liquidation(l) => {
            let side = u8::from(l.forced_order_side == Side::Sell);
            w.u8(LIQUIDATION).u8(side).i128(l.price_scaled).i128(l.quantity_scaled);
        }
        MarketFact::Funding(f) => {
            w.u8(FUNDING);
            decimal(&mut w, f.rate);
            time(&mut w, f.next_funding_time);
        }
        MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled }) => {
            w.u8(OPEN_INTEREST_BASE).i128(*quantity_scaled);
        }
        MarketFact::OpenInterest(OpenInterest::QuoteValue { value }) => {
            w.u8(OPEN_INTEREST_QUOTE);
            decimal(&mut w, *value);
        }
        MarketFact::MarkPrice { mark_price_scaled } => {
            w.u8(MARK_PRICE).i128(*mark_price_scaled);
        }
        MarketFact::IndexPrice { index_price_scaled } => {
            w.u8(INDEX_PRICE).i128(*index_price_scaled);
        }
        MarketFact::LongShortRatio(r) => {
            w.u8(LONG_SHORT_RATIO);
            decimal(&mut w, r.long_share);
            decimal(&mut w, r.short_share);
            w.i128(r.period_ns);
        }
        MarketFact::Resolution { won } => {
            w.u8(RESOLUTION).u8(u8::from(*won));
        }
        MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_) => return Err(FactError::NotAFact),
    }
    Ok(w.finish())
}

/// Decode a payload. An unknown class, a newer version or an unknown kind is refused, never
/// skipped.
pub fn decode(payload: &[u8]) -> Result<FactUpdate, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.class != ClassId::MarketFact {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    let instrument = InstrumentId::new(r.str()?);
    let exchange_time = read_time(&mut r)?;
    let received_at_ns = r.i128()?;
    let fact = read_fact(&mut r)?;
    r.finish()?;
    Ok(FactUpdate { instrument, exchange_time, received_at_ns, fact })
}

fn read_fact(r: &mut Reader<'_>) -> Result<MarketFact, DecodeError> {
    Ok(match r.u8()? {
        TRADE => MarketFact::Trade(Trade {
            trade_id: r.str()?.to_string(),
            aggressor: read_aggressor(r)?,
            price_scaled: r.i128()?,
            quantity_scaled: r.i128()?,
        }),
        LIQUIDATION => MarketFact::Liquidation(Liquidation {
            forced_order_side: match r.u8()? {
                0 => Side::Buy,
                1 => Side::Sell,
                other => {
                    return Err(DecodeError::InvalidValue { field: "side", found: other.into() });
                }
            },
            price_scaled: r.i128()?,
            quantity_scaled: r.i128()?,
        }),
        FUNDING => MarketFact::Funding(Funding {
            rate: read_decimal(r)?,
            next_funding_time: read_time(r)?,
        }),
        OPEN_INTEREST_BASE => {
            MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled: r.i128()? })
        }
        OPEN_INTEREST_QUOTE => {
            MarketFact::OpenInterest(OpenInterest::QuoteValue { value: read_decimal(r)? })
        }
        MARK_PRICE => MarketFact::MarkPrice { mark_price_scaled: r.i128()? },
        INDEX_PRICE => MarketFact::IndexPrice { index_price_scaled: r.i128()? },
        LONG_SHORT_RATIO => MarketFact::LongShortRatio(LongShortRatio {
            long_share: read_decimal(r)?,
            short_share: read_decimal(r)?,
            period_ns: r.i128()?,
        }),
        RESOLUTION => MarketFact::Resolution {
            won: match r.u8()? {
                0 => false,
                1 => true,
                other => {
                    return Err(DecodeError::InvalidValue { field: "won", found: other.into() });
                }
            },
        },
        other => return Err(DecodeError::InvalidValue { field: "fact", found: other.into() }),
    })
}

fn time(w: &mut Writer, time: ExchangeTime) {
    match time {
        ExchangeTime::At { at_ns } => w.u8(1).i128(at_ns),
        ExchangeTime::NotPublished => w.u8(0).i128(0),
    };
}

fn read_time(r: &mut Reader<'_>) -> Result<ExchangeTime, DecodeError> {
    let stamped = r.u8()?;
    let at_ns = r.i128()?;
    match stamped {
        0 => Ok(ExchangeTime::NotPublished),
        1 => Ok(ExchangeTime::At { at_ns }),
        other => Err(DecodeError::InvalidValue { field: "exchange_time", found: other.into() }),
    }
}

fn decimal(w: &mut Writer, value: ScaledDecimal) {
    w.i128(value.value_scaled).u8(value.scale);
}

fn read_decimal(r: &mut Reader<'_>) -> Result<ScaledDecimal, DecodeError> {
    Ok(ScaledDecimal { value_scaled: r.i128()?, scale: r.u8()? })
}

fn aggressor_code(aggressor: Aggressor) -> u8 {
    match aggressor {
        Aggressor::NotPublished => 0,
        Aggressor::Buyer => 1,
        Aggressor::Seller => 2,
    }
}

fn read_aggressor(r: &mut Reader<'_>) -> Result<Aggressor, DecodeError> {
    match r.u8()? {
        0 => Ok(Aggressor::NotPublished),
        1 => Ok(Aggressor::Buyer),
        2 => Ok(Aggressor::Seller),
        other => Err(DecodeError::InvalidValue { field: "aggressor", found: other.into() }),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/signal/facts_tests.rs"]
mod tests;
