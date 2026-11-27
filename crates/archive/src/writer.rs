// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The archive writer: buffers rows for one segment, seals it to complete files, and indexes them.
//!
//! A segment covers one window of received time, bounded by a row count, so where it ends is a
//! function of the records alone and a replay seals the same files. A sealed file is written to a
//! temporary name, synced, renamed and synced again before its manifest line is appended. A crash
//! at any point leaves either no trace or an unlisted file that the next open quarantines.

use std::path::{Path, PathBuf};

use bijective_model::market::{MarketFact, MarketRecord};

use crate::columns::{book_columns, gap_columns};
use crate::error::ArchiveError;
use crate::facts::{FactRow, fact_columns, fact_row};
use crate::manifest::{self, Entry, FileEntry};
use crate::rows::{BookRow, GapRow, book_rows};
use crate::table::{TableMeta, encode};

/// One minute of received time per segment: short enough that a crash costs little to rebuild,
/// long enough that a quiet instrument does not produce a file per handful of rows.
pub const SEGMENT_WINDOW_NS: i64 = 60_000_000_000;

/// The memory bound on buffered rows. At under 200 bytes a row this holds a segment under 20 MiB.
pub const SEGMENT_MAX_ROWS: usize = 100_000;

/// Above this a decimal no longer fits the 18 digits of a 64-bit decimal column.
const MAX_SCALE: u8 = 18;

const LOCK: &str = ".writer.lock";
pub(crate) const QUARANTINE: &str = "quarantine";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveConfig {
    pub root: PathBuf,
    pub venue: String,
    pub price_scale: u8,
    pub quantity_scale: u8,
}

impl ArchiveConfig {
    pub fn venue_dir(&self) -> PathBuf {
        self.root.join(&self.venue)
    }

    pub(crate) fn meta(&self, table: &'static str) -> TableMeta {
        TableMeta {
            table,
            venue: self.venue.clone(),
            price_scale: self.price_scale,
            quantity_scale: self.quantity_scale,
        }
    }
}

/// What opening found: whether the previous session ended cleanly, and what it set aside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenReport {
    pub previous_session_closed: bool,
    pub quarantined: Vec<String>,
}

/// Counts an operator reads. `buffered_rows` is what a crash now would leave to the journal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveCounts {
    pub sealed_files: u64,
    pub sealed_rows: u64,
    pub buffered_rows: u64,
    /// Records with no table here yet (trades, funding, open interest and the like). Their frames
    /// are in the journal, so a later archive that has the table is rebuilt from it.
    pub not_archived: u64,
}

/// The tables, in the order a segment's files are written and read.
pub(crate) const TABLES: [&str; 3] = ["book", "facts", "gaps"];

pub struct ArchiveWriter {
    config: ArchiveConfig,
    dir: PathBuf,
    book: Vec<BookRow>,
    gaps: Vec<GapRow>,
    facts: Vec<FactRow>,
    window_start: Option<i64>,
    counts: ArchiveCounts,
    locked: bool,
}

impl ArchiveWriter {
    /// Takes the venue's lock, sets aside unlisted files and records the session's start.
    /// A stale lock is not broken: it means a writer died, and an operator decides.
    pub fn open(config: ArchiveConfig, at_ns: i128) -> Result<(Self, OpenReport), ArchiveError> {
        for scale in [config.price_scale, config.quantity_scale] {
            if scale > MAX_SCALE {
                return Err(ArchiveError::ScaleUnsupported { scale });
            }
        }
        let dir = config.venue_dir();
        std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::io(&dir, e))?;
        take_lock(&dir)?;
        let mut writer = Self {
            config,
            dir,
            book: Vec::new(),
            gaps: Vec::new(),
            facts: Vec::new(),
            window_start: None,
            counts: ArchiveCounts::default(),
            locked: true,
        };
        let report = writer.recover()?;
        manifest::append(&writer.dir, &Entry::Opened { at_ns: at_ns.to_string() })?;
        Ok((writer, report))
    }

    fn recover(&mut self) -> Result<OpenReport, ArchiveError> {
        let entries = manifest::read(&self.dir)?;
        let live = manifest::live_files(&entries);
        let mut quarantined = Vec::new();
        for table in TABLES {
            for path in files_under(&self.dir.join(table))? {
                let relative = relative_to(&self.dir, &path);
                if path.extension().is_some_and(|e| e == "tmp") {
                    std::fs::remove_file(&path).map_err(|e| ArchiveError::io(&path, e))?;
                } else if !live.contains_key(&relative) {
                    set_aside(&self.dir, &path, &relative)?;
                    quarantined.push(relative);
                }
            }
        }
        Ok(OpenReport {
            previous_session_closed: manifest::last_session_closed(&entries),
            quarantined,
        })
    }

    /// Precondition: the record's frame was journaled at `journal_seq`.
    pub fn append_record(
        &mut self,
        record: &MarketRecord,
        journal_seq: u64,
        record_in_frame: u32,
    ) -> Result<(), ArchiveError> {
        if !matches!(record.fact, MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_)) {
            let Some(row) = fact_row(record, journal_seq, record_in_frame)? else {
                self.counts.not_archived += 1;
                return Ok(());
            };
            self.admit(row.received_at_ns, 1)?;
            self.facts.push(row);
            self.counts.buffered_rows = self.buffered();
            return Ok(());
        }
        let rows = book_rows(record, journal_seq, record_in_frame)?;
        self.admit(rows[0].received_at_ns, rows.len())?;
        self.book.extend(rows);
        self.counts.buffered_rows = self.buffered();
        Ok(())
    }

    /// Seals the open segment first when `received_at_ns` falls in another window or `rows` more
    /// would pass the bound, then opens the record's window.
    fn admit(&mut self, received_at_ns: i64, rows: usize) -> Result<(), ArchiveError> {
        let window = received_at_ns.div_euclid(SEGMENT_WINDOW_NS) * SEGMENT_WINDOW_NS;
        let held = self.book.len() + self.facts.len();
        let full = held + rows > SEGMENT_MAX_ROWS;
        if held > 0 && (self.window_start != Some(window) || full) {
            self.seal_segment()?;
        }
        self.window_start = Some(window);
        Ok(())
    }

    fn buffered(&self) -> u64 {
        (self.book.len() + self.facts.len() + self.gaps.len()) as u64
    }

    /// Gaps join the open segment. They are few, but bounded like rows all the same.
    pub fn mark_gap(&mut self, gap: GapRow) -> Result<(), ArchiveError> {
        if self.gaps.len() >= SEGMENT_MAX_ROWS {
            self.seal_segment()?;
        }
        self.gaps.push(gap);
        self.counts.buffered_rows = self.buffered();
        Ok(())
    }

    pub fn counts(&self) -> ArchiveCounts {
        self.counts
    }

    /// Writes whatever is buffered as complete, listed files. Nothing buffered writes nothing.
    pub fn seal_segment(&mut self) -> Result<(), ArchiveError> {
        let Some(name) = self.segment_name() else {
            return Ok(());
        };
        if !self.book.is_empty() {
            let meta = self.config.meta("book");
            let rows = self.book.len() as u64;
            self.write_file(&meta, &name, &[book_columns(&self.book)], rows)?;
        }
        if !self.facts.is_empty() {
            let meta = self.config.meta("facts");
            let rows = self.facts.len() as u64;
            self.write_file(&meta, &name, &[fact_columns(&self.facts)], rows)?;
        }
        if !self.gaps.is_empty() {
            let meta = self.config.meta("gaps");
            let rows = self.gaps.len() as u64;
            self.write_file(&meta, &name, &[gap_columns(&self.gaps)], rows)?;
        }
        self.book.clear();
        self.facts.clear();
        self.gaps.clear();
        self.window_start = None;
        self.counts.buffered_rows = 0;
        Ok(())
    }

    /// `date=YYYY-MM-DD/seg-<window>-<first journal entry>`: deterministic, and unique because
    /// no two segments start at one journal entry.
    fn segment_name(&self) -> Option<String> {
        let first_seq = [
            self.book.first().map(|b| b.journal_seq),
            self.facts.first().map(|f| f.journal_seq),
            self.gaps.first().map(|g| g.journal_seq),
        ]
        .into_iter()
        .flatten()
        .min()?;
        let window = self.window_start.unwrap_or_else(|| {
            let at = self.gaps.first().map_or(0, |g| g.at_ns);
            at.div_euclid(SEGMENT_WINDOW_NS) * SEGMENT_WINDOW_NS
        });
        let date = &bijective_common::clock::as_utc(i128::from(window))[..10];
        Some(format!("date={date}/seg-{window:020}-{first_seq:020}.parquet"))
    }

    fn write_file(
        &mut self,
        meta: &TableMeta,
        name: &str,
        groups: &[Vec<crate::columns::Column>],
        rows: u64,
    ) -> Result<(), ArchiveError> {
        let relative = format!("{}/{name}", meta.table);
        let path = self.dir.join(&relative);
        let bytes = encode(meta, groups, &path)?;
        place(&path, &bytes)?;
        let file = FileEntry {
            table: meta.table.to_string(),
            path: relative,
            rows,
            bytes: bytes.len() as u64,
            sha256: manifest::sha256_hex(&bytes),
        };
        manifest::append(&self.dir, &Entry::Sealed { file })?;
        self.counts.sealed_files += 1;
        self.counts.sealed_rows += rows;
        Ok(())
    }

    /// Seals what is buffered, records the session's end and releases the lock.
    pub fn close(mut self, at_ns: i128) -> Result<ArchiveCounts, ArchiveError> {
        self.seal_segment()?;
        manifest::append(&self.dir, &Entry::Closed { at_ns: at_ns.to_string() })?;
        release_lock(&self.dir);
        self.locked = false;
        Ok(self.counts)
    }
}

/// Released on any exit, so a leftover lock always means the process died.
impl Drop for ArchiveWriter {
    fn drop(&mut self) {
        if self.locked {
            release_lock(&self.dir);
        }
    }
}

pub(crate) fn take_lock(dir: &Path) -> Result<(), ArchiveError> {
    let lock = dir.join(LOCK);
    match std::fs::File::create_new(&lock) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(ArchiveError::Locked { path: lock })
        }
        Err(e) => Err(ArchiveError::io(&lock, e)),
    }
}

pub(crate) fn release_lock(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(LOCK));
}

/// Temporary name, sync, rename, sync the directory: the final name only ever holds a whole file.
pub(crate) fn place(path: &Path, bytes: &[u8]) -> Result<(), ArchiveError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
    let tmp = path.with_extension("parquet.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| ArchiveError::io(&tmp, e))?;
        f.write_all(bytes).map_err(|e| ArchiveError::io(&tmp, e))?;
        f.sync_all().map_err(|e| ArchiveError::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| ArchiveError::io(path, e))?;
    std::fs::File::open(parent).and_then(|d| d.sync_all()).map_err(|e| ArchiveError::io(parent, e))
}

/// Every file below `dir`, sorted, so recovery acts in the same order on every host.
pub(crate) fn files_under(dir: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(ArchiveError::io(&d, e)),
        };
        for entry in entries {
            let path = entry.map_err(|e| ArchiveError::io(&d, e))?.path();
            if path.is_dir() { pending.push(path) } else { out.push(path) }
        }
    }
    out.sort();
    Ok(out)
}

pub(crate) fn relative_to(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Moved, never deleted: an unlisted file may be the only evidence of what a crash interrupted.
fn set_aside(dir: &Path, path: &Path, relative: &str) -> Result<(), ArchiveError> {
    let target = dir.join(QUARANTINE).join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
    }
    std::fs::rename(path, &target).map_err(|e| ArchiveError::io(path, e))
}
