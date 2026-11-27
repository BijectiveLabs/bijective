// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The research archive: normalised market records as Parquet with zstd, written per venue.
//!
//! Layout under `<root>/<venue>/`: `book/date=YYYY-MM-DD/` (one row per book level), `facts/`
//! (one row per trade, liquidation, funding rate, open interest, mark or index price), `gaps/`
//! (where the record may be incomplete) and `manifest.jsonl` (every file, its hash and row count).
//! The archive is a projection of the data engine's frame journal: a rebuild from the journal
//! produces the same bytes. Design notes: `docs/concepts/market_archive.md`.

pub mod compact;
pub mod error;
pub mod facts;
pub mod manifest;
pub mod read;
pub mod rows;
pub mod writer;

mod columns;
mod table;

pub use compact::{Compacted, compact};
pub use error::ArchiveError;
pub use facts::{FactKind, FactRow, FactSide};
pub use read::{
    JournalPoint, NewestFacts, VerifyReport, all_book_rows, all_fact_rows, all_gap_rows,
    newest_fact_rows, verify,
};
pub use rows::{BookRow, BookUpdate, GapKind, GapRow, LevelAction, LevelRow};
pub use writer::{
    ArchiveConfig, ArchiveCounts, ArchiveWriter, OpenReport, SEGMENT_MAX_ROWS, SEGMENT_WINDOW_NS,
};

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
