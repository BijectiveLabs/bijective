// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Journal retention: the frames whose records a verified archive holds are removed from the
//! frame journal, so the journal grows with the unarchived tail instead of with all history.
//!
//! Fails closed. The journal is pruned only when the archive verifies, every archive session
//! before the current one closed, and the archive's newest entry matches the journal entry at the
//! same position. After a prune the archive is the record of the removed range, and the journal
//! can no longer rebuild it.

use std::path::Path;

use bijective_archive::manifest::{self, Entry};
use bijective_archive::{ArchiveConfig, ArchiveError, JournalPoint, verify};
use bijective_event_store::retention::{Pruned, prune_before};
use bijective_event_store::store::{StoreError, read_range};

use crate::journal::{self, JournalEntry};

#[derive(Debug)]
pub enum RetentionError {
    /// The archive did not verify. Its findings are the reason nothing was removed.
    ArchiveUnsound {
        findings: Vec<String>,
    },
    /// A session before the current one never closed: rows it buffered were lost and only the
    /// journal holds their frames, so the archive must be rebuilt before the journal is pruned.
    SessionUnclosed,
    /// The archive's newest entry does not match the journal at that position: the archive was
    /// not written from this journal.
    NotThisJournal {
        journal_seq: u64,
    },
    Archive(ArchiveError),
    Store(StoreError),
}

impl core::fmt::Display for RetentionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ArchiveUnsound { findings } => {
                write!(f, "the archive did not verify ({} finding(s))", findings.len())
            }
            Self::SessionUnclosed => write!(
                f,
                "an earlier archive session never closed; rebuild the archive from the journal first"
            ),
            Self::NotThisJournal { journal_seq } => write!(
                f,
                "the archive's entry {journal_seq} does not match this journal; it was written from another"
            ),
            Self::Archive(e) => write!(f, "archive: {e}"),
            Self::Store(e) => write!(f, "journal: {e}"),
        }
    }
}

impl std::error::Error for RetentionError {}

/// What a prune proved and removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retained {
    /// The newest journal entry the verified archive covers. The entry itself is kept.
    pub archived_through: JournalPoint,
    pub pruned: Pruned,
}

/// Removes the journal segments that end before the newest entry the verified archive covers.
/// Precondition: `archive` is the archive this store's data engine writes for `venue`.
pub fn prune_journal(
    store_root: &Path,
    venue: &str,
    archive: &ArchiveConfig,
) -> Result<Retained, RetentionError> {
    let entries = manifest::read(&archive.venue_dir()).map_err(RetentionError::Archive)?;
    if !earlier_sessions_closed(&entries) {
        return Err(RetentionError::SessionUnclosed);
    }
    let report = verify(archive).map_err(RetentionError::Archive)?;
    let through = match report.journal_through {
        Some(point) if report.is_sound() => point,
        _ => return Err(RetentionError::ArchiveUnsound { findings: report.findings }),
    };

    let stream = journal::stream(venue);
    check_same_journal(store_root, &stream, through)?;
    let pruned =
        prune_before(store_root, &stream, through.journal_seq).map_err(RetentionError::Store)?;
    Ok(Retained { archived_through: through, pruned })
}

/// Every session but the last is closed. The last may be the one still writing.
fn earlier_sessions_closed(entries: &[Entry]) -> bool {
    let mut open = false;
    for entry in entries {
        match entry {
            Entry::Opened { .. } if open => return false,
            Entry::Opened { .. } => open = true,
            Entry::Closed { .. } => open = false,
            Entry::Sealed { .. } | Entry::Compacted { .. } => {}
        }
    }
    true
}

/// The entry at the archive's newest position carries the instant the archive row recorded.
fn check_same_journal(
    store_root: &Path,
    stream: &str,
    through: JournalPoint,
) -> Result<(), RetentionError> {
    let seq = through.journal_seq;
    let mismatch = RetentionError::NotThisJournal { journal_seq: seq };
    let stored = match read_range(store_root, stream, seq, seq) {
        Ok(mut found) if found.len() == 1 => found.remove(0),
        Ok(_) => return Err(mismatch),
        Err(e) => return Err(RetentionError::Store(e)),
    };
    let entry = journal::decode(&stored.payload).map_err(|_| mismatch)?;
    let at_ns = match entry {
        JournalEntry::Frame { received_at_ns, .. } => received_at_ns,
        JournalEntry::ContinuityLost { at_ns }
        | JournalEntry::SessionStarted { at_ns }
        | JournalEntry::SessionEnded { at_ns } => at_ns,
    };
    if at_ns == i128::from(through.at_ns) {
        Ok(())
    } else {
        Err(RetentionError::NotThisJournal { journal_seq: seq })
    }
}

#[cfg(test)]
#[path = "../tests/unit/retention_tests.rs"]
mod tests;
