// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The archive's index: one JSON line per event, appended and synced, never rewritten.
//!
//! A file is part of the archive only once its line is written, and only until a compaction line
//! names it as merged. A file on disk that no line names is unlisted and is not data.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ArchiveError;

pub const MANIFEST: &str = "manifest.jsonl";

/// A sealed file: where it is relative to the venue directory, and what it must hash to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub table: String,
    pub path: String,
    pub rows: u64,
    pub bytes: u64,
    pub sha256: String,
}

/// Instants are decimal strings: a JSON number is a double in most readers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Entry {
    Opened { at_ns: String },
    Sealed { file: FileEntry },
    Compacted { file: FileEntry, from: Vec<String> },
    Closed { at_ns: String },
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Synced before returning: a line reported as written must survive a crash.
pub fn append(dir: &Path, entry: &Entry) -> Result<(), ArchiveError> {
    let path = dir.join(MANIFEST);
    let mut line = serde_json::to_string(entry)
        .map_err(|e| ArchiveError::Manifest { line: 0, detail: e.to_string() })?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| ArchiveError::io(&path, e))?;
    file.write_all(line.as_bytes()).map_err(|e| ArchiveError::io(&path, e))?;
    file.sync_data().map_err(|e| ArchiveError::io(&path, e))
}

/// An absent manifest is an empty archive. A line that does not parse is refused, never skipped.
pub fn read(dir: &Path) -> Result<Vec<Entry>, ArchiveError> {
    let path = dir.join(MANIFEST);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ArchiveError::io(&path, e)),
    };
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(n, l)| {
            serde_json::from_str(l)
                .map_err(|e| ArchiveError::Manifest { line: n + 1, detail: e.to_string() })
        })
        .collect()
}

/// The files that hold the archive now, keyed by path, in manifest order of first listing.
pub fn live_files(entries: &[Entry]) -> BTreeMap<String, (usize, FileEntry)> {
    let mut live = BTreeMap::new();
    for (order, entry) in entries.iter().enumerate() {
        match entry {
            Entry::Sealed { file } => {
                live.insert(file.path.clone(), (order, file.clone()));
            }
            Entry::Compacted { file, from } => {
                for merged in from {
                    live.remove(merged);
                }
                live.insert(file.path.clone(), (order, file.clone()));
            }
            Entry::Opened { .. } | Entry::Closed { .. } => {}
        }
    }
    live
}

/// Whether the session the manifest last opened was closed. False means it ended abnormally and
/// rows buffered at the time never reached a file. The journal still holds their frames.
pub fn last_session_closed(entries: &[Entry]) -> bool {
    let last =
        entries.iter().rev().find(|e| matches!(e, Entry::Opened { .. } | Entry::Closed { .. }));
    !matches!(last, Some(Entry::Opened { .. }))
}
