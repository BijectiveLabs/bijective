// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The archive's guarantees, each shown by a case that would break it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bijective_model::market::{
    BookDelta, BookLevel, BookSide, BookSnapshot, ExchangeTime, LevelChange, MarketFact,
    MarketRecord,
};
use bijective_model::{Sequence, VenueSymbol};

use crate::manifest::{self, Entry};
use crate::read::{live, read_book};
use crate::rows::book_rows;
use crate::writer::files_under;
use crate::*;

const MINUTE: i128 = 60_000_000_000;
/// 2026-11-03 00:00:00 UTC.
const DAY: i128 = 1_793_664_000_000_000_000;

fn root(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bl-archive-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn config(root: &Path) -> ArchiveConfig {
    ArchiveConfig {
        root: root.to_path_buf(),
        venue: "venue-a".into(),
        price_scale: 2,
        quantity_scale: 3,
    }
}

fn level(price_scaled: i128, quantity_scaled: i128) -> BookLevel {
    BookLevel { price_scaled, quantity_scaled }
}

fn snapshot(at: i128, bids: Vec<BookLevel>, asks: Vec<BookLevel>) -> MarketRecord {
    MarketRecord {
        instrument: VenueSymbol::new("BTCUSDT"),
        exchange_time: ExchangeTime::At { at_ns: at - 5_000_000 },
        received_at_ns: at,
        fact: MarketFact::BookSnapshot(BookSnapshot {
            sequence: Sequence::Numbered(7),
            bids,
            asks,
        }),
    }
}

fn delta(at: i128, changes: Vec<LevelChange>) -> MarketRecord {
    MarketRecord {
        instrument: VenueSymbol::new("BTCUSDT"),
        exchange_time: ExchangeTime::NotPublished,
        received_at_ns: at,
        fact: MarketFact::BookDelta(BookDelta { sequence: Sequence::NotNumbered, changes }),
    }
}

fn session(root: &Path, records: &[MarketRecord]) -> ArchiveCounts {
    let (mut w, _) = ArchiveWriter::open(config(root), DAY).expect("open");
    w.mark_gap(GapRow::new(0, DAY, GapKind::SessionStarted, "").unwrap()).unwrap();
    for (seq, r) in records.iter().enumerate() {
        w.append_record(r, seq as u64 + 1, 0).expect("append");
    }
    w.close(DAY + 10 * MINUTE).expect("close")
}

fn two_windows() -> Vec<MarketRecord> {
    vec![
        snapshot(
            DAY + 1,
            vec![level(10_000, 1_000), level(9_990, 2_000)],
            vec![level(10_010, 500)],
        ),
        delta(DAY + 2, vec![LevelChange::Remove { side: BookSide::Bid, price_scaled: 9_990 }]),
        delta(
            DAY + MINUTE + 1,
            vec![LevelChange::Set {
                side: BookSide::Ask,
                price_scaled: 10_020,
                quantity_scaled: 1,
            }],
        ),
    ]
}

#[test]
fn a_book_record_becomes_one_row_per_level_and_a_removal_has_no_quantity() {
    let rows = book_rows(&two_windows()[0], 3, 1).unwrap();
    assert_eq!(rows.len(), 3);
    let levels: Vec<_> = rows.iter().map(|r| r.level.unwrap()).collect();
    assert_eq!(
        (levels[0].index, levels[0].side, levels[0].price_scaled),
        (0, BookSide::Bid, 10_000)
    );
    assert_eq!((levels[2].index, levels[2].side), (2, BookSide::Ask));
    assert!(rows.iter().all(|r| r.journal_seq == 3 && r.record_in_frame == 1));

    let removal = book_rows(&two_windows()[1], 4, 0).unwrap();
    assert_eq!(removal[0].level.unwrap().action, LevelAction::Remove);
    assert_eq!(removal[0].level.unwrap().quantity_scaled, None);
}

/// An empty snapshot clears the book. Dropping it would leave a reader holding the old one.
#[test]
fn an_empty_snapshot_is_one_row_with_no_level() {
    let rows = book_rows(&snapshot(DAY, vec![], vec![]), 1, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].level, None);
    assert_eq!(rows[0].update, BookUpdate::Snapshot);
}

#[test]
fn a_fact_with_no_table_and_a_figure_too_wide_are_refused() {
    let mut mark = snapshot(DAY, vec![], vec![]);
    mark.fact = MarketFact::MarkPrice { mark_price_scaled: 1 };
    assert!(matches!(book_rows(&mark, 1, 0), Err(ArchiveError::NoTable { class: "mark price" })));

    let wide = snapshot(DAY, vec![level(i128::from(i64::MAX) + 1, 1)], vec![]);
    assert!(matches!(book_rows(&wide, 1, 0), Err(ArchiveError::OutOfRange { field: "price", .. })));
}

#[test]
fn what_is_written_reads_back_row_for_row() {
    let r = root("roundtrip");
    let counts = session(&r, &two_windows());
    assert_eq!(counts.sealed_files, 3, "two book segments and one gaps file");

    let mut expected = Vec::new();
    for (seq, rec) in two_windows().iter().enumerate() {
        expected.extend(book_rows(rec, seq as u64 + 1, 0).unwrap());
    }
    assert_eq!(all_book_rows(&config(&r)).unwrap(), expected);
    let gaps = all_gap_rows(&config(&r)).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].kind, GapKind::SessionStarted);
}

/// The same records give the same bytes, so a rebuild from the journal can be checked by digest.
#[test]
fn the_same_records_produce_byte_identical_files() {
    let (a, b) = (root("det-a"), root("det-b"));
    session(&a, &two_windows());
    session(&b, &two_windows());
    let files = |r: &Path| -> Vec<(String, Vec<u8>)> {
        let dir = config(r).venue_dir();
        files_under(&dir.join("book"))
            .unwrap()
            .into_iter()
            .chain(files_under(&dir.join("gaps")).unwrap())
            .map(|p| (crate::writer::relative_to(&dir, &p), std::fs::read(&p).unwrap()))
            .collect()
    };
    let (fa, fb) = (files(&a), files(&b));
    assert_eq!(fa.len(), 3);
    assert_eq!(fa, fb);
}

#[test]
fn a_segment_is_one_minute_of_received_time_in_its_utc_date() {
    let r = root("windows");
    session(&r, &two_windows());
    let paths: Vec<String> =
        live(&config(&r), "book").unwrap().into_iter().map(|f| f.path).collect();
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().all(|p| p.starts_with("book/date=2026-11-03/seg-")), "{paths:?}");
}

#[test]
fn a_segment_never_exceeds_its_row_bound() {
    let r = root("bound");
    let wide: Vec<BookLevel> = (0..60_000).map(|i| level(1_000_000 - i, 1)).collect();
    let records = vec![snapshot(DAY + 1, wide.clone(), vec![]), snapshot(DAY + 2, wide, vec![])];
    session(&r, &records);
    let files = live(&config(&r), "book").unwrap();
    assert_eq!(files.len(), 2, "two records of 60,000 rows cannot share a 100,000-row segment");
    assert!(files.iter().all(|f| f.rows as usize <= SEGMENT_MAX_ROWS));
}

#[test]
fn a_sound_archive_verifies_and_each_kind_of_damage_is_found() {
    let r = root("verify");
    session(&r, &two_windows());
    let report = verify(&config(&r)).unwrap();
    assert!(report.is_sound(), "{report:?}");
    assert_eq!((report.files, report.book_rows, report.gap_rows), (3, 5, 1));
    let through = report.journal_through.expect("a sound archive covers a journal entry");
    assert_eq!(through.journal_seq, 3, "the last record was journaled at entry 3");

    let dir = config(&r).venue_dir();
    let first = live(&config(&r), "book").unwrap().remove(0).path;
    let mut bytes = std::fs::read(dir.join(&first)).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(dir.join(&first), &bytes).unwrap();
    std::fs::write(dir.join("book/date=2026-11-03/stray.parquet"), b"PAR1").unwrap();

    let findings = verify(&config(&r)).unwrap().findings;
    assert!(findings.iter().any(|f| f.contains("does not hash")), "{findings:?}");
    assert!(findings.iter().any(|f| f.contains("stray.parquet")), "{findings:?}");
}

/// A pass over nothing is not a pass.
#[test]
fn an_empty_archive_does_not_verify_as_sound() {
    let report = verify(&config(&root("empty"))).unwrap();
    assert!(!report.is_sound());
}

#[test]
fn a_file_of_another_scale_is_refused_on_read() {
    let r = root("scale");
    session(&r, &two_windows());
    let path = live(&config(&r), "book").unwrap().remove(0).path;
    let other = ArchiveConfig { price_scale: 4, ..config(&r) };
    assert!(matches!(read_book(&other, &path), Err(ArchiveError::Corrupt { .. })));
}

/// A writer that dies leaves its lock to the drop, its unsealed rows to the journal, and the
/// manifest saying the session never closed.
#[test]
fn after_an_abnormal_end_the_next_open_says_so_and_sets_aside_what_is_unlisted() {
    let r = root("crash");
    {
        let (mut w, _) = ArchiveWriter::open(config(&r), DAY).unwrap();
        w.append_record(&two_windows()[0], 1, 0).unwrap();
        w.seal_segment().unwrap();
        w.append_record(&two_windows()[1], 2, 0).unwrap();
    }
    let dir = config(&r).venue_dir();
    std::fs::write(dir.join("book/date=2026-11-03/seg-half.parquet.tmp"), b"partial").unwrap();
    std::fs::write(dir.join("book/date=2026-11-03/seg-unlisted.parquet"), b"PAR1").unwrap();

    let (w, report) = ArchiveWriter::open(config(&r), DAY + MINUTE).unwrap();
    assert!(!report.previous_session_closed);
    assert_eq!(report.quarantined, vec!["book/date=2026-11-03/seg-unlisted.parquet".to_string()]);
    assert!(!dir.join("book/date=2026-11-03/seg-half.parquet.tmp").exists());
    assert!(dir.join("quarantine/book/date=2026-11-03/seg-unlisted.parquet").exists());
    w.close(DAY + 2 * MINUTE).unwrap();
    assert!(verify(&config(&r)).unwrap().is_sound());
}

#[test]
fn a_second_writer_and_a_stale_lock_are_refused() {
    let r = root("lock");
    let (w, _) = ArchiveWriter::open(config(&r), DAY).unwrap();
    assert!(matches!(ArchiveWriter::open(config(&r), DAY), Err(ArchiveError::Locked { .. })));
    w.close(DAY + 1).unwrap();

    std::fs::write(config(&r).venue_dir().join(".writer.lock"), b"").unwrap();
    assert!(matches!(ArchiveWriter::open(config(&r), DAY), Err(ArchiveError::Locked { .. })));
}

#[test]
fn compaction_merges_a_partition_and_proves_it_before_removing_anything() {
    let r = root("compact");
    session(&r, &two_windows());
    let cfg = config(&r);
    let before = all_book_rows(&cfg).unwrap();
    let inputs: Vec<String> = live(&cfg, "book").unwrap().into_iter().map(|f| f.path).collect();

    let done = compact(&cfg, "book", "2026-11-03").unwrap();
    assert_eq!((done.inputs, done.rows), (2, 5));
    assert_eq!(all_book_rows(&cfg).unwrap(), before);
    assert_eq!(live(&cfg, "book").unwrap().len(), 1);
    assert!(inputs.iter().all(|p| !cfg.venue_dir().join(p).exists()));
    assert!(verify(&cfg).unwrap().is_sound());

    assert!(matches!(
        compact(&cfg, "book", "2026-11-03"),
        Err(ArchiveError::NothingToCompact { .. })
    ));
}

#[test]
fn compaction_is_refused_while_a_writer_holds_the_venue() {
    let r = root("compact-locked");
    session(&r, &two_windows());
    let (w, _) = ArchiveWriter::open(config(&r), DAY + MINUTE).unwrap();
    assert!(matches!(compact(&config(&r), "book", "2026-11-03"), Err(ArchiveError::Locked { .. })));
    w.close(DAY + 2 * MINUTE).unwrap();
}

/// Superseded inputs left by a compaction interrupted after its manifest line are unlisted.
#[test]
fn inputs_a_compaction_superseded_are_not_live() {
    let entries = vec![
        Entry::Sealed { file: file("book/a") },
        Entry::Sealed { file: file("book/b") },
        Entry::Compacted { file: file("book/c"), from: vec!["book/a".into(), "book/b".into()] },
    ];
    let live: Vec<String> = manifest::live_files(&entries).into_keys().collect();
    assert_eq!(live, vec!["book/c".to_string()]);
}

fn file(path: &str) -> manifest::FileEntry {
    manifest::FileEntry {
        table: "book".into(),
        path: path.into(),
        rows: 1,
        bytes: 1,
        sha256: String::new(),
    }
}

#[test]
fn a_scale_beyond_the_column_is_refused_at_open() {
    let cfg = ArchiveConfig { price_scale: 19, ..config(&root("wide-scale")) };
    assert!(matches!(
        ArchiveWriter::open(cfg, DAY),
        Err(ArchiveError::ScaleUnsupported { scale: 19 })
    ));
}
