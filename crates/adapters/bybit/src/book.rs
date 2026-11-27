// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Incremental order book with the venue sequence rule.

use std::collections::BTreeMap;

use bijective_venue::contract::Continuity;

/// Scaled price to scaled quantity.
type Side = BTreeMap<i128, i128>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookError {
    /// A delta arrived before any snapshot. The book has nothing to apply it to.
    NotInitialised,
    /// The sequence did not advance.
    SequenceWentBackwards { last: u64, received: u64 },
    /// The same sequence arrived twice.
    DuplicateSequence { seq: u64 },
}

impl core::fmt::Display for BookError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BookError::NotInitialised => write!(f, "a delta arrived before any snapshot"),
            BookError::SequenceWentBackwards { last, received } => {
                write!(f, "sequence went backwards: held {last}, received {received}")
            }
            BookError::DuplicateSequence { seq } => write!(f, "sequence {seq} arrived twice"),
        }
    }
}

impl std::error::Error for BookError {}

/// A price level as it arrives on the wire, already converted to platform units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub price_scaled: i128,
    /// Zero means **remove this level**, not "a level with no size".
    pub quantity_scaled: i128,
}

#[derive(Debug, Clone, Default)]
pub struct L2Book {
    bids: Side,
    asks: Side,
    last_seq: Option<u64>,
}

impl L2Book {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a delta may be applied at all.
    pub fn is_initialised(&self) -> bool {
        self.last_seq.is_some()
    }

    /// Replace the book from a snapshot.
    pub fn apply_snapshot(&mut self, bids: &[Level], asks: &[Level], seq: u64) {
        self.bids.clear();
        self.asks.clear();
        for l in bids {
            if l.quantity_scaled != 0 {
                self.bids.insert(l.price_scaled, l.quantity_scaled);
            }
        }
        for l in asks {
            if l.quantity_scaled != 0 {
                self.asks.insert(l.price_scaled, l.quantity_scaled);
            }
        }
        self.last_seq = Some(seq);
    }

    /// Checks `seq > last`, not `last + 1`: the sequence is per connection and skips between one symbol's deltas.
    pub fn apply_delta(
        &mut self,
        bids: &[Level],
        asks: &[Level],
        seq: u64,
    ) -> Result<(), BookError> {
        let Some(last) = self.last_seq else {
            return Err(BookError::NotInitialised);
        };
        if seq == last {
            self.invalidate();
            return Err(BookError::DuplicateSequence { seq });
        }
        if seq < last {
            self.invalidate();
            return Err(BookError::SequenceWentBackwards { last, received: seq });
        }

        for l in bids {
            apply_level(&mut self.bids, *l);
        }
        for l in asks {
            apply_level(&mut self.asks, *l);
        }
        self.last_seq = Some(seq);
        Ok(())
    }

    /// A book that has taken a bad delta is not a book. The caller resubscribes.
    fn invalidate(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.last_seq = None;
    }

    /// Best bid, highest first.
    pub fn best_bid(&self) -> Option<(i128, i128)> {
        self.bids.iter().next_back().map(|(p, q)| (*p, *q))
    }

    /// Best ask, lowest first.
    pub fn best_ask(&self) -> Option<(i128, i128)> {
        self.asks.iter().next().map(|(p, q)| (*p, *q))
    }

    /// Reported, not acted on: a stale side can cross an otherwise sound book that the next delta repairs.
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => bid >= ask,
            _ => false,
        }
    }

    /// Both sides best first: bids by descending price, asks by ascending price.
    pub fn levels(
        &self,
    ) -> (impl Iterator<Item = (i128, i128)> + '_, impl Iterator<Item = (i128, i128)> + '_) {
        (self.bids.iter().rev().map(|(p, q)| (*p, *q)), self.asks.iter().map(|(p, q)| (*p, *q)))
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.bids.len(), self.asks.len())
    }

    /// The extent is unknown by construction once the book has been cleared.
    pub fn continuity_after_loss(&self) -> Continuity {
        if self.is_initialised() { Continuity::Unbroken } else { Continuity::GapOfUnknownExtent }
    }
}

fn apply_level(side: &mut Side, level: Level) {
    if level.quantity_scaled == 0 {
        side.remove(&level.price_scaled);
    } else {
        side.insert(level.price_scaled, level.quantity_scaled);
    }
}
