// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Market records flattened to table rows. Pure, so the flattening is tested without a file.
//!
//! A book record becomes one row per level or change. A record with none becomes one row with
//! the level columns absent, because an empty snapshot clears the book and must not vanish.
//! Every row names the journal entry its frame was written at, so a row is traceable to the
//! exact bytes the venue sent.

use std::sync::Arc;

use bijective_model::Sequence;
use bijective_model::market::{
    BookLevel, BookSide, ExchangeTime, LevelChange, MarketFact, MarketRecord,
};

use crate::error::ArchiveError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookUpdate {
    Snapshot,
    Delta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelAction {
    Set,
    Remove,
}

/// One level of one book record. `quantity_scaled` is absent exactly when the action removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelRow {
    pub index: u32,
    pub side: BookSide,
    pub action: LevelAction,
    pub price_scaled: i64,
    pub quantity_scaled: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookRow {
    pub journal_seq: u64,
    /// Which record of its frame this row belongs to. A frame may carry several.
    pub record_in_frame: u32,
    pub received_at_ns: i64,
    pub exchange_time_ns: Option<i64>,
    pub instrument: Arc<str>,
    pub sequence: Option<u64>,
    pub update: BookUpdate,
    pub level: Option<LevelRow>,
}

/// Why the record of a venue may be incomplete from this instant on, or where it starts and ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    SessionStarted,
    /// Absent before a later `SessionStarted` means the previous session ended abnormally.
    SessionEnded,
    /// Books were dropped. Each instrument is incomplete until its next snapshot.
    ContinuityLost,
    /// A frame was received and could not become records.
    FrameRefused,
}

impl GapKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::SessionStarted => "session-started",
            Self::SessionEnded => "session-ended",
            Self::ContinuityLost => "continuity-lost",
            Self::FrameRefused => "frame-refused",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::SessionStarted, Self::SessionEnded, Self::ContinuityLost, Self::FrameRefused]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapRow {
    pub journal_seq: u64,
    pub at_ns: i64,
    pub kind: GapKind,
    pub detail: String,
}

impl GapRow {
    pub fn new(
        journal_seq: u64,
        at_ns: i128,
        kind: GapKind,
        detail: impl Into<String>,
    ) -> Result<Self, ArchiveError> {
        Ok(Self { journal_seq, at_ns: narrow("gap time", at_ns)?, kind, detail: detail.into() })
    }
}

/// Postcondition: at least one row, all sharing the record's instants and identity.
pub fn book_rows(
    record: &MarketRecord,
    journal_seq: u64,
    record_in_frame: u32,
) -> Result<Vec<BookRow>, ArchiveError> {
    let (update, sequence, levels) = match &record.fact {
        MarketFact::BookSnapshot(s) => {
            (BookUpdate::Snapshot, s.sequence, snapshot_levels(&s.bids, &s.asks)?)
        }
        MarketFact::BookDelta(d) => (BookUpdate::Delta, d.sequence, delta_levels(&d.changes)?),
        other => return Err(ArchiveError::NoTable { class: fact_name(other) }),
    };

    let template = BookRow {
        journal_seq,
        record_in_frame,
        received_at_ns: narrow("received time", record.received_at_ns)?,
        exchange_time_ns: match record.exchange_time {
            ExchangeTime::At { at_ns } => Some(narrow("exchange time", at_ns)?),
            ExchangeTime::NotPublished => None,
        },
        instrument: Arc::from(record.instrument.as_str()),
        sequence: match sequence {
            Sequence::Numbered(n) => Some(n),
            Sequence::NotNumbered => None,
        },
        update,
        level: None,
    };
    if levels.is_empty() {
        return Ok(vec![template]);
    }
    Ok(levels.into_iter().map(|level| BookRow { level: Some(level), ..template.clone() }).collect())
}

fn snapshot_levels(bids: &[BookLevel], asks: &[BookLevel]) -> Result<Vec<LevelRow>, ArchiveError> {
    let sides =
        bids.iter().map(|l| (BookSide::Bid, l)).chain(asks.iter().map(|l| (BookSide::Ask, l)));
    sides
        .enumerate()
        .map(|(index, (side, l))| {
            Ok(LevelRow {
                index: index_of(index)?,
                side,
                action: LevelAction::Set,
                price_scaled: narrow("price", l.price_scaled)?,
                quantity_scaled: Some(narrow("quantity", l.quantity_scaled)?),
            })
        })
        .collect()
}

fn delta_levels(changes: &[LevelChange]) -> Result<Vec<LevelRow>, ArchiveError> {
    changes
        .iter()
        .enumerate()
        .map(|(index, change)| {
            let (side, action, price, quantity) = match *change {
                LevelChange::Set { side, price_scaled, quantity_scaled } => (
                    side,
                    LevelAction::Set,
                    price_scaled,
                    Some(narrow("quantity", quantity_scaled)?),
                ),
                LevelChange::Remove { side, price_scaled } => {
                    (side, LevelAction::Remove, price_scaled, None)
                }
            };
            Ok(LevelRow {
                index: index_of(index)?,
                side,
                action,
                price_scaled: narrow("price", price)?,
                quantity_scaled: quantity,
            })
        })
        .collect()
}

fn index_of(index: usize) -> Result<u32, ArchiveError> {
    u32::try_from(index)
        .map_err(|_| ArchiveError::OutOfRange { field: "level index", value: index as i128 })
}

/// Refused, never truncated: a figure that does not fit is a defect, not a rounding.
pub(crate) fn narrow(field: &'static str, value: i128) -> Result<i64, ArchiveError> {
    i64::try_from(value).map_err(|_| ArchiveError::OutOfRange { field, value })
}

fn fact_name(fact: &MarketFact) -> &'static str {
    match fact {
        MarketFact::BookSnapshot(_) => "book snapshot",
        MarketFact::BookDelta(_) => "book delta",
        MarketFact::Trade(_) => "trade",
        MarketFact::Liquidation(_) => "liquidation",
        MarketFact::Funding(_) => "funding",
        MarketFact::OpenInterest(_) => "open interest",
        MarketFact::MarkPrice { .. } => "mark price",
        MarketFact::IndexPrice { .. } => "index price",
        MarketFact::LongShortRatio(_) => "long/short ratio",
        MarketFact::Resolution { .. } => "resolution",
    }
}
