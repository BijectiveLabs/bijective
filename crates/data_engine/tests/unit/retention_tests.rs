// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Retention against a real journal and a real archive, written directly so each precondition
//! can be broken on its own.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bijective_archive::{ArchiveConfig, ArchiveWriter, GapKind, GapRow};
use bijective_event_store::format::{ENTRY_HEADER_LEN, SEGMENT_HEADER_LEN};
use bijective_event_store::store::{StoreError, StreamWriter, read_range};
use bijective_model::Sequence;
use bijective_model::market::{BookLevel, BookSnapshot, ExchangeTime, MarketFact, MarketRecord};
use bijective_venue::contract::VenueSymbol;

use super::{RetentionError, prune_journal};
use crate::journal::{self, JournalEntry};

const VENUE: &str = "venue-a";
const T0: i128 = 1_793_664_000_000_000_000;

fn root(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bl-retention-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn archive(root: &Path) -> ArchiveConfig {
    ArchiveConfig {
        root: root.join("archive"),
        venue: VENUE.into(),
        price_scale: 2,
        quantity_scale: 3,
    }
}

fn frame(seq: u64) -> JournalEntry {
    JournalEntry::Frame { received_at_ns: T0 + i128::from(seq), frame: vec![b'x'; 32] }
}

/// Entry 0 starts the session, entries 1 to 9 are frames. Two entries to a journal segment.
fn journal(root: &Path) {
    let payloads: Vec<Vec<u8>> = std::iter::once(JournalEntry::SessionStarted { at_ns: T0 })
        .chain((1..10).map(frame))
        .map(|e| journal::encode(&e).unwrap())
        .collect();
    let two = (SEGMENT_HEADER_LEN + 2 * (ENTRY_HEADER_LEN + payloads[1].len())) as u64;
    let mut w =
        StreamWriter::open(root, &journal::stream(VENUE)).unwrap().with_max_segment_bytes(two);
    for p in &payloads {
        w.append(p).unwrap();
    }
}

fn record(seq: u64) -> MarketRecord {
    MarketRecord {
        instrument: VenueSymbol::new("AAA"),
        exchange_time: ExchangeTime::At { at_ns: T0 },
        received_at_ns: T0 + i128::from(seq),
        fact: MarketFact::BookSnapshot(BookSnapshot {
            sequence: Sequence::NotNumbered,
            bids: vec![BookLevel { price_scaled: 100, quantity_scaled: 1 }],
            asks: vec![],
        }),
    }
}

/// An archive session holding the frames at `seqs`. Closed unless `close` is false.
fn session(root: &Path, seqs: impl IntoIterator<Item = u64>, close: bool) {
    let (mut w, _) = ArchiveWriter::open(archive(root), T0).unwrap();
    w.mark_gap(GapRow::new(0, T0, GapKind::SessionStarted, "").unwrap()).unwrap();
    for seq in seqs {
        w.append_record(&record(seq), seq, 0).unwrap();
    }
    if close {
        w.close(T0 + 100).unwrap();
    }
}

#[test]
fn the_journal_is_pruned_up_to_the_newest_archived_entry_and_no_further() {
    let r = root("prune");
    journal(&r);
    session(&r, 1..=6, true);

    let retained = prune_journal(&r, VENUE, &archive(&r)).unwrap();
    assert_eq!(retained.archived_through.journal_seq, 6);
    // Segments 0-1, 2-3 and 4-5 end before 6. 6-7 holds the archived entry and is kept.
    assert_eq!((retained.pruned.segments, retained.pruned.horizon), (3, Some(6)));

    let stream = journal::stream(VENUE);
    assert_eq!(read_range(&r, &stream, 6, 9).unwrap().len(), 4);
    assert!(matches!(read_range(&r, &stream, 0, 9), Err(StoreError::Pruned { horizon: 6, .. })));
    assert!(bijective_archive::verify(&archive(&r)).unwrap().is_sound());
}

#[test]
fn an_earlier_session_that_never_closed_refuses_the_prune() {
    let r = root("unclosed");
    journal(&r);
    session(&r, 1..=3, false);
    session(&r, 4..=6, true);

    assert!(matches!(prune_journal(&r, VENUE, &archive(&r)), Err(RetentionError::SessionUnclosed)));
    assert_eq!(read_range(&r, &journal::stream(VENUE), 0, 9).unwrap().len(), 10);
}

#[test]
fn the_session_still_writing_does_not_refuse_the_prune() {
    let r = root("current");
    journal(&r);
    session(&r, 1..=4, true);
    let (mut live, _) = ArchiveWriter::open(archive(&r), T0).unwrap();
    live.append_record(&record(5), 5, 0).unwrap();
    live.seal_segment().unwrap();

    let retained = prune_journal(&r, VENUE, &archive(&r)).unwrap();
    assert_eq!(retained.archived_through.journal_seq, 5);
    drop(live);
}

#[test]
fn an_archive_that_does_not_verify_removes_nothing() {
    let r = root("unsound");
    journal(&r);
    session(&r, 1..=6, true);
    let book = archive(&r).venue_dir().join("book");
    let file = files_below(&book).remove(0);
    let mut bytes = std::fs::read(&file).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(&file, bytes).unwrap();

    assert!(matches!(
        prune_journal(&r, VENUE, &archive(&r)),
        Err(RetentionError::ArchiveUnsound { .. })
    ));
    assert_eq!(read_range(&r, &journal::stream(VENUE), 0, 9).unwrap().len(), 10);
}

#[test]
fn an_archive_written_from_another_journal_removes_nothing() {
    let r = root("other");
    journal(&r);
    let (mut w, _) = ArchiveWriter::open(archive(&r), T0).unwrap();
    let mut moved = record(6);
    moved.received_at_ns += 1_000;
    w.append_record(&moved, 6, 0).unwrap();
    w.close(T0 + 100).unwrap();

    assert!(matches!(
        prune_journal(&r, VENUE, &archive(&r)),
        Err(RetentionError::NotThisJournal { journal_seq: 6 })
    ));
    assert_eq!(read_range(&r, &journal::stream(VENUE), 0, 9).unwrap().len(), 10);
}

fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { pending.push(path) } else { out.push(path) }
        }
    }
    out.sort();
    out
}
