// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Turns venue frames into normalised book tops.
//! Opens nothing. Frames come from a socket or a recording.

use std::collections::BTreeMap;

use bijective_venue::contract::market::{Aggressor, Funding, OpenInterest, Trade};
use bijective_venue::contract::market::{BookLevel, BookSnapshot, ExchangeTime};
use bijective_venue::contract::{
    BookTop, FeedError, MarketFact, MarketFeed, MarketRecord, Offered, Sequence, VenueSymbol,
    check_record,
};
use bijective_venue::normalise::Scale;

use crate::message::{AssetContext, Level, VenueMessage, WireTrade, parse};

/// Held whole and replaced whole: the venue sends full books and numbers nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Top {
    bid: Option<Level>,
    ask: Option<Level>,
    bid_depth: u32,
    ask_depth: u32,
    venue_ts_ms: u64,
}

/// BTreeMap: iteration order is part of what a replay reproduces.
pub struct Feed {
    books: BTreeMap<String, Top>,
    /// The last context per coin: the venue restates all of it, so only a change is a new fact.
    contexts: BTreeMap<String, AssetContext>,
    price: Scale,
    quantity: Scale,
    accepted: u64,
}

impl Feed {
    #[must_use]
    pub fn new(price: Scale, quantity: Scale) -> Self {
        Self { books: BTreeMap::new(), contexts: BTreeMap::new(), price, quantity, accepted: 0 }
    }

    /// Live path. `offer` is the recorded path that parses first, so a frame is parsed once.
    /// Every book on this venue is whole, so each one is a snapshot. The record is checked
    /// before the book is replaced, so a refused book leaves the previous one standing.
    pub fn observe(
        &mut self,
        msg: &VenueMessage,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        if let VenueMessage::Refused { detail } = msg {
            // The error channel names no cause: report, never match.
            return Err(FeedError::Unreadable { detail: detail.clone() });
        }
        match msg {
            VenueMessage::Book { symbol, bids, asks, venue_ts_ms } => {
                self.observe_book(symbol, (bids, asks), *venue_ts_ms, received_at_ns)
            }
            VenueMessage::Trades { trades } => self.observe_trades(trades, received_at_ns),
            VenueMessage::AssetContext { symbol, context } => {
                self.observe_context(symbol, context, received_at_ns)
            }
            _ => Ok(Offered::default()),
        }
    }

    fn observe_book(
        &mut self,
        symbol: &str,
        (bids, asks): (&[Level], &[Level]),
        venue_ts_ms: u64,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let record = MarketRecord {
            instrument: VenueSymbol::new(symbol),
            exchange_time: ExchangeTime::from_ms(venue_ts_ms),
            received_at_ns,
            fact: MarketFact::BookSnapshot(BookSnapshot {
                sequence: Sequence::NotNumbered,
                bids: bids.iter().map(level).collect(),
                asks: asks.iter().map(level).collect(),
            }),
        };
        check_record(&record).map_err(FeedError::RecordRefused)?;
        let records = vec![record];

        let top = Top {
            // An empty side is absent, not zero.
            bid: bids.first().copied(),
            ask: asks.first().copied(),
            bid_depth: u32::try_from(bids.len()).unwrap_or(u32::MAX),
            ask_depth: u32::try_from(asks.len()).unwrap_or(u32::MAX),
            venue_ts_ms,
        };

        self.books.insert(symbol.to_string(), top.clone());
        self.accepted += 1;

        // One side alone prices nothing.
        let (Some(bid), Some(ask)) = (top.bid, top.ask) else {
            return Ok(Offered { records, top: None });
        };

        let top = Some(BookTop {
            instrument: VenueSymbol::new(symbol),
            // Not numbered: a zero would read as the first update ever sent.
            sequence: Sequence::NotNumbered,
            venue_ts_ms: top.venue_ts_ms,
            bid_price: bid.price_scaled,
            bid_quantity: bid.quantity_scaled,
            ask_price: ask.price_scaled,
            ask_quantity: ask.quantity_scaled,
            bid_depth: top.bid_depth,
            ask_depth: top.ask_depth,
        });
        Ok(Offered { records, top })
    }

    /// Every trade is checked before any is offered, so a refused one leaves none of its frame.
    fn observe_trades(
        &mut self,
        trades: &[WireTrade],
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let mut records = Vec::with_capacity(trades.len());
        for trade in trades {
            let aggressor = if trade.taker_buys { Aggressor::Buyer } else { Aggressor::Seller };
            let record = MarketRecord {
                instrument: VenueSymbol::new(&trade.symbol),
                exchange_time: ExchangeTime::from_ms(trade.ts_ms),
                received_at_ns,
                fact: MarketFact::Trade(Trade {
                    trade_id: trade.id.clone(),
                    aggressor,
                    price_scaled: trade.price_scaled,
                    quantity_scaled: trade.quantity_scaled,
                }),
            };
            check_record(&record).map_err(FeedError::RecordRefused)?;
            records.push(record);
        }
        self.accepted += records.len() as u64;
        Ok(Offered { records, top: None })
    }

    /// The context states no time and no next funding time, and the records say so. The oracle
    /// price is the venue's own reference for the underlying, recorded as the index price.
    fn observe_context(
        &mut self,
        symbol: &str,
        context: &AssetContext,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let held = self.contexts.get(symbol);
        let changed =
            |pick: fn(&AssetContext) -> i128| held.is_none_or(|held| pick(held) != pick(context));
        let mut facts = Vec::new();
        if held.is_none_or(|held| held.funding_rate != context.funding_rate) {
            facts.push(MarketFact::Funding(Funding {
                rate: context.funding_rate,
                next_funding_time: ExchangeTime::NotPublished,
            }));
        }
        if changed(|c| c.open_interest_scaled) {
            facts.push(MarketFact::OpenInterest(OpenInterest::BaseQuantity {
                quantity_scaled: context.open_interest_scaled,
            }));
        }
        if changed(|c| c.mark_price_scaled) {
            facts.push(MarketFact::MarkPrice { mark_price_scaled: context.mark_price_scaled });
        }
        if changed(|c| c.oracle_price_scaled) {
            facts.push(MarketFact::IndexPrice { index_price_scaled: context.oracle_price_scaled });
        }

        let mut records = Vec::with_capacity(facts.len());
        for fact in facts {
            let record = MarketRecord {
                instrument: VenueSymbol::new(symbol),
                exchange_time: ExchangeTime::NotPublished,
                received_at_ns,
                fact,
            };
            check_record(&record).map_err(FeedError::RecordRefused)?;
            records.push(record);
        }
        self.contexts.insert(symbol.to_string(), context.clone());
        self.accepted += records.len() as u64;
        Ok(Offered { records, top: None })
    }
}

/// Carried in the venue's order. The record check refuses a side that is not best first.
fn level(l: &Level) -> BookLevel {
    BookLevel { price_scaled: l.price_scaled, quantity_scaled: l.quantity_scaled }
}

impl MarketFeed for Feed {
    fn offer(&mut self, frame: &[u8], received_at_ns: i128) -> Result<Offered, FeedError> {
        let msg = parse(frame, self.price, self.quantity)
            .map_err(|e| FeedError::Unreadable { detail: e.to_string() })?;

        self.observe(&msg, received_at_ns)
    }

    /// A reconnect always breaks continuity on this venue.
    fn reset(&mut self) {
        self.books.clear();
        self.contexts.clear();
    }

    fn accepted(&self) -> u64 {
        self.accepted
    }

    fn instruments(&self) -> usize {
        self.books.len()
    }
}

#[cfg(test)]
#[path = "../tests/unit/capture_tests.rs"]
mod capture_tests;
