// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Turns venue frames into normalised observations. The adapter contract ends here.
//! Opens nothing, so a socket and a recording share the same code.

use std::collections::BTreeMap;

use bijective_model::Side;
use bijective_venue::contract::market::{
    Aggressor, BookDelta, BookLevel, BookSide, BookSnapshot, ExchangeTime, Funding, LevelChange,
    Liquidation, OpenInterest, ScaledDecimal, Trade,
};
use bijective_venue::contract::{
    BookTop, FeedError, MarketFact, MarketFeed, MarketRecord, Offered, Sequence, VenueSymbol,
    check_record,
};
use bijective_venue::profile::InstrumentScales;

use crate::book::{L2Book, Level};
use crate::message::{
    BookUpdateKind, TickerFields, VenueMessage, WireLiquidation, WireTrade, parse,
};

/// What a ticker last said about funding. A delta omits what has not changed, and a funding record
/// needs the rate and the time of the next funding together.
#[derive(Default)]
struct FundingState {
    rate: Option<ScaledDecimal>,
    next_funding_ms: Option<u64>,
}

/// BTreeMap, not HashMap: replay must reproduce iteration order.
pub struct Feed {
    books: BTreeMap<String, L2Book>,
    funding: BTreeMap<String, FundingState>,
    scales: InstrumentScales,
    accepted: u64,
}

impl Feed {
    pub fn new(scales: InstrumentScales) -> Self {
        Self { books: BTreeMap::new(), funding: BTreeMap::new(), scales, accepted: 0 }
    }

    /// Takes a parsed message. `offer` is the recorded path. Both share this so a frame is parsed once.
    pub fn observe(
        &mut self,
        msg: &VenueMessage,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        match msg {
            VenueMessage::Book { symbol, kind, bids, asks, seq, ts_ms } => {
                self.observe_book(symbol, *kind, (bids, asks), (*seq, *ts_ms), received_at_ns)
            }
            VenueMessage::Trades { symbol, trades } => {
                let facts = trades.iter().map(trade_fact).collect();
                self.emit(symbol, facts, received_at_ns)
            }
            VenueMessage::Liquidations { symbol, liquidations } => {
                let facts = liquidations.iter().map(liquidation_fact).collect();
                self.emit(symbol, facts, received_at_ns)
            }
            VenueMessage::Ticker { symbol, ts_ms, fields } => {
                let facts = self.ticker_facts(symbol, *ts_ms, fields);
                self.emit(symbol, facts, received_at_ns)
            }
            _ => Ok(Offered::default()),
        }
    }

    fn observe_book(
        &mut self,
        symbol: &str,
        kind: BookUpdateKind,
        (bids, asks): (&[Level], &[Level]),
        (seq, ts_ms): (u64, u64),
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let book = self.books.entry(symbol.to_string()).or_default();
        let fact = match kind {
            BookUpdateKind::Snapshot => {
                book.apply_snapshot(bids, asks, seq);
                MarketFact::BookSnapshot(snapshot_of(book, seq))
            }
            BookUpdateKind::Delta => {
                if !book.is_initialised() {
                    return Err(FeedError::NoSnapshot { instrument: symbol.to_string() });
                }
                book.apply_delta(bids, asks, seq)
                    .map_err(|_| FeedError::SequenceBroken { instrument: symbol.to_string() })?;
                MarketFact::BookDelta(delta_of(bids, asks, seq))
            }
        };

        let record = MarketRecord {
            instrument: VenueSymbol::new(symbol),
            exchange_time: ExchangeTime::from_ms(ts_ms),
            received_at_ns,
            fact,
        };
        check_record(&record).map_err(FeedError::RecordRefused)?;

        let top = top_of(book, symbol, seq, ts_ms);
        self.accepted += 1;
        Ok(Offered { records: vec![record], top: Some(top) })
    }

    /// Every fact is checked before any is offered, so a refused one leaves none of its frame behind.
    fn emit(
        &mut self,
        symbol: &str,
        facts: Vec<(u64, MarketFact)>,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let mut records = Vec::with_capacity(facts.len());
        for (ts_ms, fact) in facts {
            let record = MarketRecord {
                instrument: VenueSymbol::new(symbol),
                exchange_time: ExchangeTime::from_ms(ts_ms),
                received_at_ns,
                fact,
            };
            check_record(&record).map_err(FeedError::RecordRefused)?;
            records.push(record);
        }
        self.accepted += records.len() as u64;
        Ok(Offered { records, top: None })
    }

    /// One fact per field the message carried. Funding is made when the rate or the time changed
    /// and both are known. The time is never invented.
    fn ticker_facts(
        &mut self,
        symbol: &str,
        ts_ms: u64,
        fields: &TickerFields,
    ) -> Vec<(u64, MarketFact)> {
        let state = self.funding.entry(symbol.to_string()).or_default();
        state.rate = fields.funding_rate.or(state.rate);
        state.next_funding_ms = fields.next_funding_ms.or(state.next_funding_ms);

        let mut facts = Vec::new();
        let funding_changed = fields.funding_rate.is_some() || fields.next_funding_ms.is_some();
        if let (true, Some(rate), Some(next_ms)) =
            (funding_changed, state.rate, state.next_funding_ms)
        {
            let next_funding_time = ExchangeTime::from_ms(next_ms);
            facts.push((ts_ms, MarketFact::Funding(Funding { rate, next_funding_time })));
        }
        if let Some(quantity_scaled) = fields.open_interest_scaled {
            let open_interest = OpenInterest::BaseQuantity { quantity_scaled };
            facts.push((ts_ms, MarketFact::OpenInterest(open_interest)));
        }
        if let Some(mark_price_scaled) = fields.mark_price_scaled {
            facts.push((ts_ms, MarketFact::MarkPrice { mark_price_scaled }));
        }
        if let Some(index_price_scaled) = fields.index_price_scaled {
            facts.push((ts_ms, MarketFact::IndexPrice { index_price_scaled }));
        }
        facts
    }
}

fn trade_fact(trade: &WireTrade) -> (u64, MarketFact) {
    let aggressor = if trade.taker_buys { Aggressor::Buyer } else { Aggressor::Seller };
    let fact = MarketFact::Trade(Trade {
        trade_id: trade.id.clone(),
        aggressor,
        price_scaled: trade.price_scaled,
        quantity_scaled: trade.quantity_scaled,
    });
    (trade.ts_ms, fact)
}

/// The venue names the liquidated position's side. The record names the forced order's, which is
/// the opposite: a long is closed by a sell.
fn liquidation_fact(liquidation: &WireLiquidation) -> (u64, MarketFact) {
    let forced_order_side = if liquidation.position_was_long { Side::Sell } else { Side::Buy };
    let fact = MarketFact::Liquidation(Liquidation {
        forced_order_side,
        price_scaled: liquidation.price_scaled,
        quantity_scaled: liquidation.quantity_scaled,
    });
    (liquidation.ts_ms, fact)
}

/// Taken from the book after the snapshot is applied, so the record is ordered and free of zero
/// levels whatever order the wire used.
fn snapshot_of(book: &L2Book, seq: u64) -> BookSnapshot {
    let (bids, asks) = book.levels();
    let level =
        |(price_scaled, quantity_scaled): (i128, i128)| BookLevel { price_scaled, quantity_scaled };
    BookSnapshot {
        sequence: Sequence::Numbered(seq),
        bids: bids.map(level).collect(),
        asks: asks.map(level).collect(),
    }
}

/// A zero quantity on the wire is the venue's removal. The record states it as one.
fn delta_of(bids: &[Level], asks: &[Level], seq: u64) -> BookDelta {
    let change = |side: BookSide| {
        move |l: &Level| {
            if l.quantity_scaled == 0 {
                LevelChange::Remove { side, price_scaled: l.price_scaled }
            } else {
                LevelChange::Set {
                    side,
                    price_scaled: l.price_scaled,
                    quantity_scaled: l.quantity_scaled,
                }
            }
        }
    };
    let changes =
        bids.iter().map(change(BookSide::Bid)).chain(asks.iter().map(change(BookSide::Ask)));
    BookDelta { sequence: Sequence::Numbered(seq), changes: changes.collect() }
}

/// An empty side reads as zero here. The depth beside it says the side is empty.
fn top_of(book: &L2Book, symbol: &str, seq: u64, ts_ms: u64) -> BookTop {
    let (bid_depth, ask_depth) = book.depth();
    let (bid_price, bid_quantity) = book.best_bid().unwrap_or((0, 0));
    let (ask_price, ask_quantity) = book.best_ask().unwrap_or((0, 0));
    BookTop {
        instrument: VenueSymbol::new(symbol),
        sequence: Sequence::Numbered(seq),
        venue_ts_ms: ts_ms,
        bid_price,
        bid_quantity,
        ask_price,
        ask_quantity,
        bid_depth: bid_depth as u32,
        ask_depth: ask_depth as u32,
    }
}

impl MarketFeed for Feed {
    fn offer(&mut self, frame: &[u8], received_at_ns: i128) -> Result<Offered, FeedError> {
        let text = core::str::from_utf8(frame)
            .map_err(|_| FeedError::Unreadable { detail: "frame is not utf-8".into() })?;
        let msg = parse(text, &self.scales)
            .map_err(|e| FeedError::Unreadable { detail: e.to_string() })?;
        self.observe(&msg, received_at_ns)
    }

    /// A resubscribed ticker starts with a snapshot, so no funding state is carried across.
    fn reset(&mut self) {
        self.books.clear();
        self.funding.clear();
    }

    fn accepted(&self) -> u64 {
        self.accepted
    }

    fn instruments(&self) -> usize {
        self.books.len()
    }
}

#[cfg(test)]
#[path = "../tests/unit/feed_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/capture_tests.rs"]
mod capture_tests;
