// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Stream messages turned into market records and tops. Messages come from a socket or a
//! recording. A quote states the whole top of book, so each one is a snapshot of one level a
//! side and nothing is carried between quotes: there is no book to go wrong. The venue numbers
//! nothing.

use bijective_venue::contract::market::{Aggressor, BookLevel, BookSnapshot, ExchangeTime, Trade};
use bijective_venue::contract::{
    BookTop, FeedError, MarketFact, MarketFeed, MarketRecord, Offered, Sequence, VenueSymbol,
    check_record,
};
use bijective_venue::normalise::Scale;

use super::message::{MarketMessage, Side, parse};

pub struct Feed {
    price: Scale,
    quantity: Scale,
    accepted: u64,
    /// Symbols a quote has been read for since the last reset.
    seen: std::collections::BTreeSet<String>,
}

fn level(side: Side) -> Vec<BookLevel> {
    if side.price_scaled > 0 && side.quantity_scaled > 0 {
        vec![BookLevel { price_scaled: side.price_scaled, quantity_scaled: side.quantity_scaled }]
    } else {
        Vec::new()
    }
}

fn exchange_time(at_ns: Option<i128>) -> ExchangeTime {
    at_ns.map_or(ExchangeTime::NotPublished, |at_ns| ExchangeTime::At { at_ns })
}

impl Feed {
    #[must_use]
    pub fn new(price: Scale, quantity: Scale) -> Self {
        Self { price, quantity, accepted: 0, seen: Default::default() }
    }

    /// Live path. `offer` is the recorded path that parses first, so a frame is parsed once.
    pub fn observe(
        &mut self,
        messages: &[MarketMessage],
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let mut offered = Offered::default();
        for message in messages {
            match message {
                MarketMessage::Quote { symbol, bid, ask, at_ns } => {
                    let record = MarketRecord {
                        instrument: VenueSymbol::new(symbol.as_str()),
                        exchange_time: exchange_time(*at_ns),
                        received_at_ns,
                        fact: MarketFact::BookSnapshot(BookSnapshot {
                            sequence: Sequence::NotNumbered,
                            bids: level(*bid),
                            asks: level(*ask),
                        }),
                    };
                    check_record(&record).map_err(FeedError::RecordRefused)?;
                    self.seen.insert(symbol.clone());
                    offered.top = top(symbol, *bid, *ask, *at_ns);
                    offered.records.push(record);
                }
                MarketMessage::Trade { symbol, trade_id, price_scaled, quantity_scaled, at_ns } => {
                    let record = MarketRecord {
                        instrument: VenueSymbol::new(symbol.as_str()),
                        exchange_time: exchange_time(*at_ns),
                        received_at_ns,
                        fact: MarketFact::Trade(Trade {
                            trade_id: trade_id.clone(),
                            aggressor: Aggressor::NotPublished,
                            price_scaled: *price_scaled,
                            quantity_scaled: *quantity_scaled,
                        }),
                    };
                    check_record(&record).map_err(FeedError::RecordRefused)?;
                    offered.records.push(record);
                }
                _ => {}
            }
        }
        self.accepted += offered.records.len() as u64;
        Ok(offered)
    }
}

/// The top a quote states, where both sides hold a level.
fn top(symbol: &str, bid: Side, ask: Side, at_ns: Option<i128>) -> Option<BookTop> {
    if level(bid).is_empty() || level(ask).is_empty() {
        return None;
    }
    Some(BookTop {
        instrument: VenueSymbol::new(symbol),
        sequence: Sequence::NotNumbered,
        venue_ts_ms: at_ns.map_or(0, |ns| u64::try_from(ns / 1_000_000).unwrap_or(0)),
        bid_price: bid.price_scaled,
        bid_quantity: bid.quantity_scaled,
        ask_price: ask.price_scaled,
        ask_quantity: ask.quantity_scaled,
        bid_depth: 1,
        ask_depth: 1,
    })
}

impl MarketFeed for Feed {
    fn offer(&mut self, frame: &[u8], received_at_ns: i128) -> Result<Offered, FeedError> {
        let messages = parse(frame, self.price, self.quantity)
            .map_err(|e| FeedError::Unreadable { detail: e.to_string() })?;
        self.observe(&messages, received_at_ns)
    }

    fn reset(&mut self) {
        self.seen.clear();
    }

    fn accepted(&self) -> u64 {
        self.accepted
    }

    fn instruments(&self) -> usize {
        self.seen.len()
    }
}
