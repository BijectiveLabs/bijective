// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Normalised market facts: what a venue published about an instrument, and when.
//!
//! Every record carries two instants. The exchange time is the venue's own stamp, used to order
//! events within a venue. The receive time is this platform's clock at the socket, used to decide
//! what was knowable at a moment. A point-in-time state needs both, so neither is optional.
//! Prices and quantities are scaled integers at the instrument's declared scales. Quantities are
//! in the instrument's base unit.

use crate::{Sequence, Side, VenueSymbol};

/// The venue's stamp on a fact, in nanoseconds since the Unix epoch.
/// A sum type, not a zero: a venue that stamps nothing has not stamped the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeTime {
    At { at_ns: i128 },
    NotPublished,
}

impl ExchangeTime {
    /// Exact: a millisecond stamp widens to nanoseconds without inventing precision it then claims.
    #[must_use]
    pub fn from_ms(ms: u64) -> Self {
        Self::At { at_ns: i128::from(ms) * 1_000_000 }
    }
}

/// A decimal figure with no instrument scale of its own, such as a rate or a share.
/// The scale travels with the value because no profile declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaledDecimal {
    pub value_scaled: i128,
    pub scale: u8,
}

/// One normalised fact about one instrument, with both of its instants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketRecord {
    pub instrument: VenueSymbol,
    pub exchange_time: ExchangeTime,
    /// The platform's clock when the frame carrying this fact was read from the socket, or the
    /// recorded arrival time on replay.
    pub received_at_ns: i128,
    pub fact: MarketFact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketFact {
    BookSnapshot(BookSnapshot),
    BookDelta(BookDelta),
    Trade(Trade),
    Liquidation(Liquidation),
    Funding(Funding),
    OpenInterest(OpenInterest),
    MarkPrice {
        mark_price_scaled: i128,
    },
    IndexPrice {
        index_price_scaled: i128,
    },
    LongShortRatio(LongShortRatio),
    /// The question an outcome belongs to resolved (decision 0002, rule 5): the outcome won, and
    /// redeems for one unit of collateral, or lost, and redeems for nothing.
    Resolution {
        won: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookSide {
    Bid,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookLevel {
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// The whole book as the venue states it. Bids run best first (descending price), asks best first
/// (ascending price). No level has zero quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookSnapshot {
    pub sequence: Sequence,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}

/// Changes to a book that a snapshot started. Valid only on the book it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookDelta {
    pub sequence: Sequence,
    pub changes: Vec<LevelChange>,
}

/// Removal is its own case: a level of zero quantity is not a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelChange {
    Set { side: BookSide, price_scaled: i128, quantity_scaled: i128 },
    Remove { side: BookSide, price_scaled: i128 },
}

/// Which side took liquidity, where the venue says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggressor {
    Buyer,
    Seller,
    NotPublished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trade {
    /// The venue's identity for the execution. A duplicate delivery is recognised by it.
    pub trade_id: String,
    pub aggressor: Aggressor,
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// A forced closure. The side is that of the forced order, not of the position it closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liquidation {
    pub forced_order_side: Side,
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// A funding rate per funding interval, signed: positive means longs pay shorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Funding {
    pub rate: ScaledDecimal,
    pub next_funding_time: ExchangeTime,
}

/// Open interest in the unit the venue publishes. The two are not converted into each other here,
/// because the conversion needs a price the venue did not state with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenInterest {
    BaseQuantity { quantity_scaled: i128 },
    QuoteValue { value: ScaledDecimal },
}

/// Shares of accounts holding long and short positions over a venue-defined period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LongShortRatio {
    pub long_share: ScaledDecimal,
    pub short_share: ScaledDecimal,
    pub period_ns: i128,
}
