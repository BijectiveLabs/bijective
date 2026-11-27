// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The book the simulated venue matches against: the recorded book, less what simulated orders
//! have already taken from it.
//!
//! Liquidity a simulated fill consumed stays consumed until the venue's own record next restates
//! that level, so one displayed quantity is never filled twice.

use std::collections::BTreeMap;

use bijective_model::BookTop;
use bijective_model::market::{BookLevel, BookSide, LevelChange, MarketFact};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimBook {
    bids: BTreeMap<i128, i128>,
    asks: BTreeMap<i128, i128>,
    consumed: BTreeMap<(Sided, i128), i128>,
}

/// `BookSide` with an order, for keying consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sided {
    Bid,
    Ask,
}

fn sided(side: BookSide) -> Sided {
    match side {
        BookSide::Bid => Sided::Bid,
        BookSide::Ask => Sided::Ask,
    }
}

impl SimBook {
    /// A book fact replaces or amends what is displayed. Anything else is not a book fact.
    pub fn apply(&mut self, fact: &MarketFact) {
        match fact {
            MarketFact::BookSnapshot(s) => self.replace(&s.bids, &s.asks),
            MarketFact::BookDelta(d) => {
                for change in &d.changes {
                    self.change(*change);
                }
            }
            _ => {}
        }
    }

    /// A top of book is a snapshot of one level a side.
    pub fn apply_top(&mut self, top: &BookTop) {
        let level = |price_scaled, quantity_scaled| BookLevel { price_scaled, quantity_scaled };
        let bids: Vec<BookLevel> = (top.bid_quantity > 0)
            .then(|| level(top.bid_price, top.bid_quantity))
            .into_iter()
            .collect();
        let asks: Vec<BookLevel> = (top.ask_quantity > 0)
            .then(|| level(top.ask_price, top.ask_quantity))
            .into_iter()
            .collect();
        self.replace(&bids, &asks);
    }

    fn replace(&mut self, bids: &[BookLevel], asks: &[BookLevel]) {
        self.bids = bids.iter().map(|l| (l.price_scaled, l.quantity_scaled)).collect();
        self.asks = asks.iter().map(|l| (l.price_scaled, l.quantity_scaled)).collect();
        self.consumed.clear();
    }

    fn change(&mut self, change: LevelChange) {
        let (side, price, quantity) = match change {
            LevelChange::Set { side, price_scaled, quantity_scaled } => {
                (side, price_scaled, Some(quantity_scaled))
            }
            LevelChange::Remove { side, price_scaled } => (side, price_scaled, None),
        };
        let levels = match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        };
        match quantity {
            Some(q) => levels.insert(price, q),
            None => levels.remove(&price),
        };
        self.consumed.remove(&(sided(side), price));
    }

    /// Levels on `side`, best first, with what is left after consumption. Empty levels skipped.
    pub fn available(&self, side: BookSide) -> Vec<(i128, i128)> {
        let left = |(price, quantity): (&i128, &i128)| {
            let taken = self.consumed.get(&(sided(side), *price)).copied().unwrap_or(0);
            (*price, quantity - taken)
        };
        let levels: Vec<(i128, i128)> = match side {
            BookSide::Bid => self.bids.iter().rev().map(left).collect(),
            BookSide::Ask => self.asks.iter().map(left).collect(),
        };
        levels.into_iter().filter(|(_, q)| *q > 0).collect()
    }

    pub fn take(&mut self, side: BookSide, price_scaled: i128, quantity_scaled: i128) {
        *self.consumed.entry((sided(side), price_scaled)).or_insert(0) += quantity_scaled;
    }
}
