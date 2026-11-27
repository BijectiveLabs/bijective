// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What every market record an adapter emits must satisfy, checked where it is emitted.
//! A record that fails is refused at the adapter, never passed upward to be discovered later.

use bijective_model::market::{
    BookLevel, BookSide, ExchangeTime, Funding, LevelChange, LongShortRatio, MarketFact,
    MarketRecord, OpenInterest, ScaledDecimal,
};

use crate::profile::DataClass;

/// A decimal scale beyond this cannot be held in `i128` with even one integer digit.
const MAX_DECIMAL_SCALE: u8 = 38;

/// The first rule a record broke. Each names the field so a venue format change is diagnosable
/// from the refusal alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordDefect {
    NoInstrument,
    ReceivedBeforeEpoch {
        received_at_ns: i128,
    },
    ExchangeTimeNotPositive {
        field: &'static str,
        at_ns: i128,
    },
    NotPositive {
        field: &'static str,
        value: i128,
    },
    Negative {
        field: &'static str,
        value: i128,
    },
    /// A snapshot side out of order or holding one price twice.
    SideOutOfOrder {
        side: BookSide,
        index: usize,
    },
    ScaleTooWide {
        field: &'static str,
        scale: u8,
    },
    NoTradeIdentity,
}

impl core::fmt::Display for RecordDefect {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoInstrument => write!(f, "the record names no instrument"),
            Self::ReceivedBeforeEpoch { received_at_ns } => {
                write!(f, "received at {received_at_ns} ns, before the clock's epoch")
            }
            Self::ExchangeTimeNotPositive { field, at_ns } => {
                write!(f, "{field} is {at_ns} ns, not a venue stamp")
            }
            Self::NotPositive { field, value } => write!(f, "{field} is {value}, not positive"),
            Self::Negative { field, value } => write!(f, "{field} is {value}, below zero"),
            Self::SideOutOfOrder { side, index } => {
                write!(f, "{side:?} level {index} is not strictly behind the one before it")
            }
            Self::ScaleTooWide { field, scale } => {
                write!(f, "{field} carries scale {scale}, beyond {MAX_DECIMAL_SCALE}")
            }
            Self::NoTradeIdentity => write!(f, "the trade carries no venue identity"),
        }
    }
}

impl std::error::Error for RecordDefect {}

/// The data class a record belongs to, so an emitted record can be held against what the venue
/// declared it serves.
#[must_use]
pub fn class_of(fact: &MarketFact) -> DataClass {
    match fact {
        MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_) => DataClass::Depth,
        MarketFact::Trade(_) => DataClass::Trades,
        MarketFact::Liquidation(_) => DataClass::Liquidations,
        MarketFact::Funding(_) => DataClass::Funding,
        MarketFact::OpenInterest(_) => DataClass::OpenInterest,
        MarketFact::MarkPrice { .. } => DataClass::MarkPrice,
        MarketFact::IndexPrice { .. } => DataClass::IndexPrice,
        MarketFact::LongShortRatio(_) => DataClass::LongShortRatio,
        MarketFact::Resolution { .. } => DataClass::Resolution,
    }
}

/// Postcondition on success: every instant is a real stamp, every price positive, every
/// snapshot side strictly ordered best first, and every scale representable.
pub fn check_record(record: &MarketRecord) -> Result<(), RecordDefect> {
    if record.instrument.is_empty() {
        return Err(RecordDefect::NoInstrument);
    }
    if record.received_at_ns < 0 {
        return Err(RecordDefect::ReceivedBeforeEpoch { received_at_ns: record.received_at_ns });
    }
    check_exchange_time("exchange_time", record.exchange_time)?;
    check_fact(&record.fact)
}

fn check_fact(fact: &MarketFact) -> Result<(), RecordDefect> {
    match fact {
        MarketFact::BookSnapshot(s) => {
            check_side(&s.bids, BookSide::Bid)?;
            check_side(&s.asks, BookSide::Ask)
        }
        MarketFact::BookDelta(d) => d.changes.iter().try_for_each(check_change),
        MarketFact::Trade(t) => {
            if t.trade_id.is_empty() {
                return Err(RecordDefect::NoTradeIdentity);
            }
            positive("trade price", t.price_scaled)?;
            positive("trade quantity", t.quantity_scaled)
        }
        MarketFact::Liquidation(l) => {
            positive("liquidation price", l.price_scaled)?;
            positive("liquidation quantity", l.quantity_scaled)
        }
        MarketFact::Funding(f) => check_funding(f),
        MarketFact::OpenInterest(o) => check_open_interest(o),
        MarketFact::MarkPrice { mark_price_scaled } => positive("mark price", *mark_price_scaled),
        MarketFact::IndexPrice { index_price_scaled } => {
            positive("index price", *index_price_scaled)
        }
        MarketFact::LongShortRatio(r) => check_long_short(r),
        MarketFact::Resolution { .. } => Ok(()),
    }
}

/// Strictly ordered: a price held twice on one side is two answers to one question.
fn check_side(levels: &[BookLevel], side: BookSide) -> Result<(), RecordDefect> {
    let mut previous: Option<i128> = None;
    for (index, level) in levels.iter().enumerate() {
        positive("level price", level.price_scaled)?;
        positive("level quantity", level.quantity_scaled)?;
        let behind = match (side, previous) {
            (_, None) => true,
            (BookSide::Bid, Some(p)) => level.price_scaled < p,
            (BookSide::Ask, Some(p)) => level.price_scaled > p,
        };
        if !behind {
            return Err(RecordDefect::SideOutOfOrder { side, index });
        }
        previous = Some(level.price_scaled);
    }
    Ok(())
}

fn check_change(change: &LevelChange) -> Result<(), RecordDefect> {
    match *change {
        LevelChange::Set { price_scaled, quantity_scaled, .. } => {
            positive("level price", price_scaled)?;
            positive("level quantity", quantity_scaled)
        }
        LevelChange::Remove { price_scaled, .. } => positive("level price", price_scaled),
    }
}

/// The rate may be of either sign. Only its scale and the next stamp are checked.
fn check_funding(funding: &Funding) -> Result<(), RecordDefect> {
    check_scale("funding rate", funding.rate)?;
    check_exchange_time("next funding time", funding.next_funding_time)
}

fn check_open_interest(open_interest: &OpenInterest) -> Result<(), RecordDefect> {
    match *open_interest {
        OpenInterest::BaseQuantity { quantity_scaled } => {
            not_negative("open interest", quantity_scaled)
        }
        OpenInterest::QuoteValue { value } => {
            check_scale("open interest value", value)?;
            not_negative("open interest value", value.value_scaled)
        }
    }
}

fn check_long_short(ratio: &LongShortRatio) -> Result<(), RecordDefect> {
    for (field, share) in [("long share", ratio.long_share), ("short share", ratio.short_share)] {
        check_scale(field, share)?;
        not_negative(field, share.value_scaled)?;
    }
    positive("long/short period", ratio.period_ns)
}

fn check_exchange_time(field: &'static str, time: ExchangeTime) -> Result<(), RecordDefect> {
    match time {
        ExchangeTime::At { at_ns } if at_ns <= 0 => {
            Err(RecordDefect::ExchangeTimeNotPositive { field, at_ns })
        }
        ExchangeTime::At { .. } | ExchangeTime::NotPublished => Ok(()),
    }
}

fn check_scale(field: &'static str, decimal: ScaledDecimal) -> Result<(), RecordDefect> {
    if decimal.scale > MAX_DECIMAL_SCALE {
        return Err(RecordDefect::ScaleTooWide { field, scale: decimal.scale });
    }
    Ok(())
}

fn positive(field: &'static str, value: i128) -> Result<(), RecordDefect> {
    if value > 0 { Ok(()) } else { Err(RecordDefect::NotPositive { field, value }) }
}

fn not_negative(field: &'static str, value: i128) -> Result<(), RecordDefect> {
    if value >= 0 { Ok(()) } else { Err(RecordDefect::Negative { field, value }) }
}

#[cfg(test)]
#[path = "../../tests/unit/contract/record_tests.rs"]
mod tests;
