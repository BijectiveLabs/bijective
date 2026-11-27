// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Turns market-stream events into book records and tops. Events come from a socket or a recording.
//!
//! The venue numbers nothing, and a level a trade consumed can vanish without a change of its own
//! (measured 2026-11-15). The best prices each change states are the authority: a held level
//! better than them is removed and the removal recorded. A stated best the held book does not
//! hold cannot be rebuilt, so that book is dropped, the change refused and a snapshot asked for.

use std::collections::BTreeMap;

use bijective_venue::contract::market::{
    Aggressor, BookDelta, BookLevel, BookSide, BookSnapshot, ExchangeTime, LevelChange, Trade,
};
use bijective_venue::contract::{
    BookTop, FeedError, MarketFact, MarketFeed, MarketRecord, Offered, Sequence, VenueSymbol,
    check_record,
};
use bijective_venue::normalise::Scale;

use crate::message::{VenueEvent, WireBook, WireChange, WireTrade, parse};

/// One outcome's book, keyed by price so a change lands on its level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Book {
    bids: BTreeMap<i128, i128>,
    asks: BTreeMap<i128, i128>,
}

impl Book {
    fn best_bid(&self) -> Option<(i128, i128)> {
        self.bids.iter().next_back().map(|(p, q)| (*p, *q))
    }

    fn best_ask(&self) -> Option<(i128, i128)> {
        self.asks.iter().next().map(|(p, q)| (*p, *q))
    }

    fn side(&mut self, side: BookSide) -> &mut BTreeMap<i128, i128> {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }
}

/// BTreeMap: iteration order is part of what a replay reproduces.
pub struct Feed {
    books: BTreeMap<String, Book>,
    /// Outcomes this feed was ever sent a book for. A change to any other belongs to an outcome
    /// not subscribed here: the venue states both outcomes of a market in one change.
    known: std::collections::BTreeSet<String>,
    price: Scale,
    quantity: Scale,
    accepted: u64,
}

impl Feed {
    #[must_use]
    pub fn new(price: Scale, quantity: Scale) -> Self {
        Self { books: BTreeMap::new(), known: Default::default(), price, quantity, accepted: 0 }
    }

    /// Live path. `offer` is the recorded path that parses first, so a frame is parsed once.
    /// Every event of the frame is taken or the first refusal is returned. Records of events
    /// before it stand, because each was checked on its own.
    pub fn observe(
        &mut self,
        events: &[VenueEvent],
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let mut offered = Offered::default();
        for event in events {
            let one = match event {
                VenueEvent::Book(book) => self.observe_book(book, received_at_ns)?,
                VenueEvent::Changes { changes, ts_ms } => {
                    self.observe_changes(changes, *ts_ms, received_at_ns)?
                }
                VenueEvent::Trade(trade) => self.observe_trade(trade, received_at_ns)?,
                VenueEvent::Resolved { assets, winning, ts_ms } => {
                    self.observe_resolution(assets, winning, *ts_ms, received_at_ns)?
                }
                // A tick change is for venue assurance to observe. Nothing here records it yet.
                VenueEvent::TickSize { .. } | VenueEvent::Pong | VenueEvent::Ignored => {
                    Offered::default()
                }
            };
            offered.records.extend(one.records);
            if one.top.is_some() {
                offered.top = one.top;
            }
        }
        Ok(offered)
    }

    /// Decision 0002, rule 5: every outcome of the resolved question this feed holds is recorded as
    /// won or lost. A winner the venue does not list among the question's outcomes is refused:
    /// read as stated, every outcome would be recorded lost.
    fn observe_resolution(
        &mut self,
        assets: &[String],
        winning: &str,
        ts_ms: u64,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        if !assets.iter().any(|asset| asset == winning) {
            return Err(FeedError::Unreadable {
                detail: format!(
                    "market_resolved names {winning}, which is not one of its outcomes"
                ),
            });
        }
        let mut offered = Offered::default();
        for asset in assets.iter().filter(|asset| self.known.contains(asset.as_str())) {
            let record = MarketRecord {
                instrument: VenueSymbol::new(asset),
                exchange_time: ExchangeTime::from_ms(ts_ms),
                received_at_ns,
                fact: MarketFact::Resolution { won: asset == winning },
            };
            check_record(&record).map_err(FeedError::RecordRefused)?;
            self.accepted += 1;
            offered.records.push(record);
        }
        Ok(offered)
    }

    fn observe_book(
        &mut self,
        wire: &WireBook,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let record = MarketRecord {
            instrument: VenueSymbol::new(&wire.asset),
            exchange_time: ExchangeTime::from_ms(wire.ts_ms),
            received_at_ns,
            fact: MarketFact::BookSnapshot(BookSnapshot {
                sequence: Sequence::NotNumbered,
                bids: wire.bids.iter().map(level).collect(),
                asks: wire.asks.iter().map(level).collect(),
            }),
        };
        check_record(&record).map_err(FeedError::RecordRefused)?;
        let book = Book {
            bids: wire.bids.iter().map(|l| (l.price_scaled, l.quantity_scaled)).collect(),
            asks: wire.asks.iter().map(|l| (l.price_scaled, l.quantity_scaled)).collect(),
        };
        self.books.insert(wire.asset.clone(), book);
        self.known.insert(wire.asset.clone());
        self.accepted += 1;
        let top = self.top(&wire.asset, wire.ts_ms);
        Ok(Offered { records: vec![record], top })
    }

    /// One frame may change several outcomes. Each outcome's changes become one delta.
    fn observe_changes(
        &mut self,
        changes: &[WireChange],
        ts_ms: u64,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let mut by_asset: BTreeMap<&str, Vec<&WireChange>> = BTreeMap::new();
        for change in changes {
            by_asset.entry(change.asset.as_str()).or_default().push(change);
        }
        let mut offered = Offered::default();
        for (asset, changes) in by_asset {
            if !self.known.contains(asset) {
                continue;
            }
            let (record, top) = self.apply(asset, &changes, ts_ms, received_at_ns)?;
            offered.records.push(record);
            if top.is_some() {
                offered.top = top;
            }
        }
        Ok(offered)
    }

    fn apply(
        &mut self,
        asset: &str,
        changes: &[&WireChange],
        ts_ms: u64,
        received_at_ns: i128,
    ) -> Result<(MarketRecord, Option<BookTop>), FeedError> {
        let Some(book) = self.books.get_mut(asset) else {
            return Err(FeedError::NoSnapshot { instrument: asset.to_string() });
        };
        let mut delta = Vec::with_capacity(changes.len());
        for change in changes {
            let side = book.side(change.side);
            delta.push(if change.quantity_scaled == 0 {
                side.remove(&change.price_scaled);
                LevelChange::Remove { side: change.side, price_scaled: change.price_scaled }
            } else {
                side.insert(change.price_scaled, change.quantity_scaled);
                LevelChange::Set {
                    side: change.side,
                    price_scaled: change.price_scaled,
                    quantity_scaled: change.quantity_scaled,
                }
            });
        }
        let Some(last) = changes.last() else {
            return Err(FeedError::Unreadable { detail: "a change with no level".into() });
        };
        delta.extend(prune(book, last.best_bid_scaled, last.best_ask_scaled));
        // A stated zero is an empty side.
        let agrees = |held: Option<(i128, i128)>, stated: Option<i128>| {
            stated.is_none_or(|s| held.map_or(0, |(p, _)| p) == s)
        };
        if !agrees(book.best_bid(), last.best_bid_scaled)
            || !agrees(book.best_ask(), last.best_ask_scaled)
        {
            self.books.remove(asset);
            return Err(FeedError::SequenceBroken { instrument: asset.to_string() });
        }
        let record = MarketRecord {
            instrument: VenueSymbol::new(asset),
            exchange_time: ExchangeTime::from_ms(ts_ms),
            received_at_ns,
            fact: MarketFact::BookDelta(BookDelta {
                sequence: Sequence::NotNumbered,
                changes: delta,
            }),
        };
        check_record(&record).map_err(FeedError::RecordRefused)?;
        self.accepted += 1;
        Ok((record, self.top(asset, ts_ms)))
    }

    /// The venue does not say which side took liquidity in a trade, so none is claimed.
    fn observe_trade(
        &mut self,
        trade: &WireTrade,
        received_at_ns: i128,
    ) -> Result<Offered, FeedError> {
        let record = MarketRecord {
            instrument: VenueSymbol::new(&trade.asset),
            exchange_time: ExchangeTime::from_ms(trade.ts_ms),
            received_at_ns,
            fact: MarketFact::Trade(Trade {
                // One transaction can carry several trades of an asset. With the time, price and
                // size the identity repeats only for a repeated delivery.
                trade_id: format!(
                    "{}:{}:{}:{}",
                    trade.transaction, trade.ts_ms, trade.price_scaled, trade.quantity_scaled
                ),
                aggressor: Aggressor::NotPublished,
                price_scaled: trade.price_scaled,
                quantity_scaled: trade.quantity_scaled,
            }),
        };
        check_record(&record).map_err(FeedError::RecordRefused)?;
        self.accepted += 1;
        Ok(Offered { records: vec![record], top: None })
    }

    /// One side alone prices nothing, so a top needs both.
    fn top(&self, asset: &str, ts_ms: u64) -> Option<BookTop> {
        let book = self.books.get(asset)?;
        let ((bid, bid_qty), (ask, ask_qty)) = (book.best_bid()?, book.best_ask()?);
        Some(BookTop {
            instrument: VenueSymbol::new(asset),
            // Not numbered: a zero would read as the first update ever sent.
            sequence: Sequence::NotNumbered,
            venue_ts_ms: ts_ms,
            bid_price: bid,
            bid_quantity: bid_qty,
            ask_price: ask,
            ask_quantity: ask_qty,
            bid_depth: u32::try_from(book.bids.len()).unwrap_or(u32::MAX),
            ask_depth: u32::try_from(book.asks.len()).unwrap_or(u32::MAX),
        })
    }
}

/// Removes every held level better than the venue's stated best on its side: the venue says it
/// is gone. A stated zero empties the side. Returns the removals, best first.
fn prune(book: &mut Book, best_bid: Option<i128>, best_ask: Option<i128>) -> Vec<LevelChange> {
    let mut removed = Vec::new();
    if let Some(bid) = best_bid {
        let gone: Vec<i128> = book.bids.range(bid.saturating_add(1)..).map(|(p, _)| *p).collect();
        for price in gone.into_iter().rev() {
            book.bids.remove(&price);
            removed.push(LevelChange::Remove { side: BookSide::Bid, price_scaled: price });
        }
    }
    if let Some(ask) = best_ask {
        let below = if ask == 0 { i128::MAX } else { ask };
        let gone: Vec<i128> = book.asks.range(..below).map(|(p, _)| *p).collect();
        for price in gone {
            book.asks.remove(&price);
            removed.push(LevelChange::Remove { side: BookSide::Ask, price_scaled: price });
        }
    }
    removed
}

fn level(l: &crate::message::Level) -> BookLevel {
    BookLevel { price_scaled: l.price_scaled, quantity_scaled: l.quantity_scaled }
}

impl MarketFeed for Feed {
    fn offer(&mut self, frame: &[u8], received_at_ns: i128) -> Result<Offered, FeedError> {
        let events = parse(frame, self.price, self.quantity)
            .map_err(|e| FeedError::Unreadable { detail: e.to_string() })?;
        self.observe(&events, received_at_ns)
    }

    /// A reconnect always breaks continuity on this venue.
    fn reset(&mut self) {
        self.books.clear();
    }

    fn accepted(&self) -> u64 {
        self.accepted
    }

    fn instruments(&self) -> usize {
        self.books.len()
    }
}
