// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Compaction: one partition's sealed segments merged into one file, proven equal before the
//! segments are removed.
//!
//! Order of steps, each safe to interrupt: write the merged file under its final name (through a
//! temporary one), read it back and compare every row with its inputs, append the manifest line
//! that supersedes the inputs, then delete them. Interrupted before the line, the merged file is
//! unlisted and the next open quarantines it. After the line, the inputs are unlisted.

use crate::columns::{Column, book_columns, book_rows_from, gap_columns, gap_rows_from};
use crate::error::ArchiveError;
use crate::facts::{fact_columns, fact_rows_from};
use crate::manifest::{self, Entry, FileEntry};
use crate::read::{live, read_book, read_facts, read_gaps};
use crate::table::{decode, encode};
use crate::writer::{ArchiveConfig, place, release_lock, take_lock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compacted {
    pub path: String,
    pub inputs: usize,
    pub rows: u64,
}

/// Precondition: no writer holds the venue (the lock is taken, so a running recorder refuses it).
/// `date` is `YYYY-MM-DD`. A partition with fewer than two live files has nothing to merge.
pub fn compact(
    config: &ArchiveConfig,
    table: &'static str,
    date: &str,
) -> Result<Compacted, ArchiveError> {
    let dir = config.venue_dir();
    take_lock(&dir)?;
    let done = merge(config, table, date);
    release_lock(&dir);
    done
}

fn merge(
    config: &ArchiveConfig,
    table: &'static str,
    date: &str,
) -> Result<Compacted, ArchiveError> {
    let dir = config.venue_dir();
    let partition = format!("{table}/date={date}/");
    let inputs: Vec<FileEntry> =
        live(config, table)?.into_iter().filter(|f| f.path.starts_with(&partition)).collect();
    if inputs.len() < 2 {
        return Err(ArchiveError::NothingToCompact { partition });
    }

    let groups =
        inputs.iter().map(|f| columns_of(config, table, &f.path)).collect::<Result<Vec<_>, _>>()?;
    let first = inputs[0].path.rsplit('/').next().unwrap_or("seg").trim_start_matches("seg-");
    let relative = format!("{partition}compacted-{first}");
    let path = dir.join(&relative);
    let meta = config.meta(table);
    let bytes = encode(&meta, &groups, &path)?;
    place(&path, &bytes)?;

    let rows = prove_equal(config, table, &inputs, &relative)?;
    let file = FileEntry {
        table: table.to_string(),
        path: relative.clone(),
        rows,
        bytes: bytes.len() as u64,
        sha256: manifest::sha256_hex(&bytes),
    };
    let from = inputs.iter().map(|f| f.path.clone()).collect();
    manifest::append(&dir, &Entry::Compacted { file, from })?;
    for f in &inputs {
        let input = dir.join(&f.path);
        std::fs::remove_file(&input).map_err(|e| ArchiveError::io(&input, e))?;
    }
    Ok(Compacted { path: relative, inputs: inputs.len(), rows })
}

/// Re-encoded from the rows rather than copied as bytes, so the merged file is checked by the same
/// reader every consumer uses.
fn columns_of(
    config: &ArchiveConfig,
    table: &'static str,
    relative: &str,
) -> Result<Vec<Column>, ArchiveError> {
    let path = config.venue_dir().join(relative);
    let corrupt = |e| ArchiveError::corrupt(&path, e);
    let groups = decode(&config.meta(table), &path)?;
    Ok(match table {
        "book" => {
            let rows = groups.iter().map(|g| book_rows_from(g)).collect::<Result<Vec<_>, _>>();
            book_columns(&rows.map_err(corrupt)?.concat())
        }
        "facts" => {
            let rows = groups.iter().map(|g| fact_rows_from(g)).collect::<Result<Vec<_>, _>>();
            fact_columns(&rows.map_err(corrupt)?.concat())
        }
        _ => {
            let rows = groups.iter().map(|g| gap_rows_from(g)).collect::<Result<Vec<_>, _>>();
            gap_columns(&rows.map_err(corrupt)?.concat())
        }
    })
}

fn prove_equal(
    config: &ArchiveConfig,
    table: &'static str,
    inputs: &[FileEntry],
    merged: &str,
) -> Result<u64, ArchiveError> {
    let rows = match table {
        "book" => same_rows(inputs, merged, |p| read_book(config, p))?,
        "facts" => same_rows(inputs, merged, |p| read_facts(config, p))?,
        _ => same_rows(inputs, merged, |p| read_gaps(config, p))?,
    };
    // Left unlisted on refusal: the next open quarantines it, and the inputs stay the archive.
    rows.ok_or_else(|| {
        ArchiveError::corrupt(
            &config.venue_dir().join(merged),
            "the merged file does not hold its inputs' rows",
        )
    })
}

/// The merged rows, counted, when they equal the inputs' rows in order. `None` otherwise.
fn same_rows<T: PartialEq>(
    inputs: &[FileEntry],
    merged: &str,
    read: impl Fn(&str) -> Result<Vec<T>, ArchiveError>,
) -> Result<Option<u64>, ArchiveError> {
    let mut expected = Vec::new();
    for f in inputs {
        expected.extend(read(&f.path)?);
    }
    let found = read(merged)?;
    Ok((expected == found).then_some(found.len() as u64))
}
