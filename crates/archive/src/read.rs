// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reading the archive back, and verifying it against its manifest.
//!
//! Verification is what makes the archive evidence: every listed file exists, hashes to its
//! recorded digest, reads under its table's schema and holds its recorded row count, and no
//! file outside the quarantine is unlisted. A pass over an empty archive is reported as empty,
//! not as sound.

use std::path::Path;

use crate::columns::{Column, book_rows_from, gap_rows_from};
use crate::error::ArchiveError;
use crate::facts::{FactRow, fact_rows_from};
use crate::manifest::{self, FileEntry};
use crate::rows::{BookRow, GapRow};
use crate::table::{declared_scales, decode};
use crate::writer::{ArchiveConfig, TABLES, files_under, relative_to};

fn groups(
    config: &ArchiveConfig,
    table: &'static str,
    relative: &str,
) -> Result<Vec<Vec<Column>>, ArchiveError> {
    decode(&config.meta(table), &config.venue_dir().join(relative))
}

pub fn read_book(config: &ArchiveConfig, relative: &str) -> Result<Vec<BookRow>, ArchiveError> {
    let path = config.venue_dir().join(relative);
    let mut rows = Vec::new();
    for g in groups(config, "book", relative)? {
        rows.extend(book_rows_from(&g).map_err(|e| ArchiveError::corrupt(&path, e))?);
    }
    Ok(rows)
}

pub fn read_gaps(config: &ArchiveConfig, relative: &str) -> Result<Vec<GapRow>, ArchiveError> {
    let path = config.venue_dir().join(relative);
    let mut rows = Vec::new();
    for g in groups(config, "gaps", relative)? {
        rows.extend(gap_rows_from(&g).map_err(|e| ArchiveError::corrupt(&path, e))?);
    }
    Ok(rows)
}

pub fn read_facts(config: &ArchiveConfig, relative: &str) -> Result<Vec<FactRow>, ArchiveError> {
    let path = config.venue_dir().join(relative);
    let mut rows = Vec::new();
    for g in groups(config, "facts", relative)? {
        rows.extend(fact_rows_from(&g).map_err(|e| ArchiveError::corrupt(&path, e))?);
    }
    Ok(rows)
}

/// Every fact row of the archive, in the order of `live`.
pub fn all_fact_rows(config: &ArchiveConfig) -> Result<Vec<FactRow>, ArchiveError> {
    let mut rows = Vec::new();
    for f in live(config, "facts")? {
        rows.extend(read_facts(config, &f.path)?);
    }
    Ok(rows)
}

/// The newest facts of one venue, with the scales its files declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewestFacts {
    pub price_scale: u8,
    pub quantity_scale: u8,
    /// In journal order.
    pub rows: Vec<FactRow>,
}

/// The facts of `<root>/<venue>`'s newest files, for a reader that knows the venue and not its
/// scales. Files are taken newest first while their listed rows stay within `row_budget`. The
/// newest is taken whatever its size, so a venue with facts never reads as empty. `None` when the
/// venue has no facts file. Every file is decoded against the newest one's declarations, so a
/// file of another scale is refused as corrupt.
pub fn newest_fact_rows(
    root: &Path,
    venue: &str,
    row_budget: u64,
) -> Result<Option<NewestFacts>, ArchiveError> {
    let mut config = ArchiveConfig {
        root: root.to_path_buf(),
        venue: venue.to_string(),
        price_scale: 0,
        quantity_scale: 0,
    };
    let mut files = live(&config, "facts")?;
    files.sort_by(|a, b| chronological(&a.path).cmp(&chronological(&b.path)));
    let Some(newest) = files.last() else { return Ok(None) };
    (config.price_scale, config.quantity_scale) =
        declared_scales(&config.venue_dir().join(&newest.path))?;

    let mut taken = 0_u64;
    let mut rows = Vec::new();
    for (n, f) in files.iter().rev().enumerate() {
        if n > 0 && taken + f.rows > row_budget {
            break;
        }
        taken += f.rows;
        rows.extend(read_facts(&config, &f.path)?);
    }
    rows.sort_by_key(|r| (r.journal_seq, r.record_in_frame));
    Ok(Some(NewestFacts {
        price_scale: config.price_scale,
        quantity_scale: config.quantity_scale,
        rows,
    }))
}

/// Time order of `<table>/date=<day>/<seg|compacted>-<window>-<first entry>.parquet`. Manifest
/// order is not it: a compaction lists an old partition after newer segments. Both numbers are
/// zero-padded, so text order after the prefix is time order.
fn chronological(path: &str) -> (&str, &str) {
    let (partition, name) = path.rsplit_once('/').unwrap_or(("", path));
    let name =
        name.strip_prefix("seg-").or_else(|| name.strip_prefix("compacted-")).unwrap_or(name);
    (partition, name)
}

/// The live files of one table, in the order the manifest first lists them. A compaction lists
/// its partition again, after whatever was sealed before it.
pub fn live(config: &ArchiveConfig, table: &str) -> Result<Vec<FileEntry>, ArchiveError> {
    let entries = manifest::read(&config.venue_dir())?;
    let mut files: Vec<(usize, FileEntry)> =
        manifest::live_files(&entries).into_values().filter(|(_, f)| f.table == table).collect();
    files.sort_by_key(|(order, _)| *order);
    Ok(files.into_iter().map(|(_, f)| f).collect())
}

/// Every book row of the archive, in the order of `live`.
pub fn all_book_rows(config: &ArchiveConfig) -> Result<Vec<BookRow>, ArchiveError> {
    let mut rows = Vec::new();
    for f in live(config, "book")? {
        rows.extend(read_book(config, &f.path)?);
    }
    Ok(rows)
}

pub fn all_gap_rows(config: &ArchiveConfig) -> Result<Vec<GapRow>, ArchiveError> {
    let mut rows = Vec::new();
    for f in live(config, "gaps")? {
        rows.extend(read_gaps(config, &f.path)?);
    }
    Ok(rows)
}

/// The newest journal entry a verified file holds rows from, with the instant its row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JournalPoint {
    pub journal_seq: u64,
    pub at_ns: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub files: u64,
    pub book_rows: u64,
    pub gap_rows: u64,
    pub fact_rows: u64,
    /// Sealing flushes both tables in journal order, so every entry before this one that produced
    /// a row is in a verified file. `None` when no file verified.
    pub journal_through: Option<JournalPoint>,
    /// Each a sentence naming the file and what is wrong with it. Empty means sound.
    pub findings: Vec<String>,
}

impl VerifyReport {
    pub fn is_sound(&self) -> bool {
        self.findings.is_empty() && self.files > 0
    }
}

pub fn verify(config: &ArchiveConfig) -> Result<VerifyReport, ArchiveError> {
    let dir = config.venue_dir();
    let entries = manifest::read(&dir)?;
    let live = manifest::live_files(&entries);
    let mut report = VerifyReport::default();

    for (_, file) in live.values() {
        report.files += 1;
        match check_file(config, &dir, file) {
            Ok((rows, newest)) => {
                match file.table.as_str() {
                    "book" => report.book_rows += rows,
                    "facts" => report.fact_rows += rows,
                    _ => report.gap_rows += rows,
                }
                report.journal_through = report.journal_through.max(newest);
            }
            Err(finding) => report.findings.push(finding),
        }
    }
    for table in TABLES {
        for path in files_under(&dir.join(table))? {
            let relative = relative_to(&dir, &path);
            if !live.contains_key(&relative) {
                report.findings.push(format!("{relative} is on disk and in no manifest line"));
            }
        }
    }
    if report.files == 0 {
        report.findings.push(format!("{} holds no archived file", dir.display()));
    }
    Ok(report)
}

fn check_file(
    config: &ArchiveConfig,
    dir: &Path,
    file: &FileEntry,
) -> Result<(u64, Option<JournalPoint>), String> {
    let path = dir.join(&file.path);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", file.path))?;
    if bytes.len() as u64 != file.bytes || manifest::sha256_hex(&bytes) != file.sha256 {
        return Err(format!("{} does not hash to its manifest digest", file.path));
    }
    let points: Vec<JournalPoint> = match file.table.as_str() {
        "book" => read_book(config, &file.path).map(|rows| {
            rows.iter()
                .map(|r| JournalPoint { journal_seq: r.journal_seq, at_ns: r.received_at_ns })
                .collect()
        }),
        "facts" => read_facts(config, &file.path).map(|rows| {
            rows.iter()
                .map(|r| JournalPoint { journal_seq: r.journal_seq, at_ns: r.received_at_ns })
                .collect()
        }),
        "gaps" => read_gaps(config, &file.path).map(|rows| {
            rows.iter()
                .map(|g| JournalPoint { journal_seq: g.journal_seq, at_ns: g.at_ns })
                .collect()
        }),
        other => return Err(format!("{} names table {other}", file.path)),
    }
    .map_err(|e| e.to_string())?;
    let rows = points.len() as u64;
    if rows != file.rows {
        return Err(format!(
            "{} holds {rows} rows, its manifest line says {}",
            file.path, file.rows
        ));
    }
    Ok((rows, points.into_iter().max()))
}
