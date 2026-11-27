// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reading on from a cursor: only what was appended, verified, and never a quiet hole.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::cursor::*;
use super::format::*;
use super::store::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn root(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("bijective-event-store-cursor-{}-{name}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn seqs(entries: &[Entry]) -> Vec<u64> {
    entries.iter().map(|e| e.seq).collect()
}

fn seg0(root: &Path) -> PathBuf {
    root.join("orders").join("000000000000.seg")
}

#[test]
fn a_cursor_returns_only_what_was_appended_after_it() {
    let r = root("on");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    for p in [b"one", b"two", b"333"] {
        w.append(p).unwrap();
    }
    let (first, cursor) = read_after(&r, "orders", None).unwrap();
    assert_eq!(seqs(&first), [0, 1, 2]);
    assert_eq!(cursor.map(|c| c.next_seq()), Some(3));

    w.append(b"four").unwrap();
    w.append(b"five").unwrap();
    let (more, cursor) = read_after(&r, "orders", cursor).unwrap();
    assert_eq!(seqs(&more), [3, 4]);
    assert_eq!(more[1].payload, b"five");

    let (none, again) = read_after(&r, "orders", cursor).unwrap();
    assert!(none.is_empty());
    assert_eq!(again, cursor, "a quiet stream leaves the cursor where it was");
}

#[test]
fn a_cursor_follows_the_stream_across_segments() {
    let r = root("rotate");
    let mut w = StreamWriter::open(&r, "orders")
        .unwrap()
        .with_max_segment_bytes((SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + 8) as u64);
    for n in 0..4u8 {
        w.append(&[n; 8]).unwrap();
    }
    let (first, cursor) = read_after(&r, "orders", None).unwrap();
    assert_eq!(seqs(&first), [0, 1, 2, 3]);
    for n in 4..7u8 {
        w.append(&[n; 8]).unwrap();
    }
    let (more, _) = read_after(&r, "orders", cursor).unwrap();
    assert_eq!(seqs(&more), [4, 5, 6]);
    let whole = read_range(&r, "orders", 0, u64::MAX).unwrap();
    assert_eq!([first, more].concat(), whole, "the same entries as one whole read");
}

/// Damage after the cursor is found by the read that reaches it.
#[test]
fn damage_after_the_cursor_stops_the_read() {
    let r = root("damage");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    w.append(b"one").unwrap();
    let (_, cursor) = read_after(&r, "orders", None).unwrap();
    w.append(b"two").unwrap();
    drop(w);

    let mut bytes = fs::read(seg0(&r)).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(seg0(&r), &bytes).unwrap();
    assert!(matches!(read_after(&r, "orders", cursor), Err(StoreError::Quarantined { .. })));
}

/// A torn tail is the crash signature: not an entry, not an error, and read once it completes.
#[test]
fn a_torn_tail_is_left_for_the_next_read() {
    let r = root("torn");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    w.append(b"one").unwrap();
    drop(w);
    let (_, cursor) = read_after(&r, "orders", None).unwrap();
    OpenOptions::new().append(true).open(seg0(&r)).unwrap().write_all(b"EN\0").unwrap();
    let (none, held) = read_after(&r, "orders", cursor).unwrap();
    assert!(none.is_empty());
    assert_eq!(held, cursor);
}

#[test]
fn a_segment_shorter_than_the_cursor_was_rewritten() {
    let r = root("shrunk");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    w.append(b"one").unwrap();
    w.append(b"two").unwrap();
    drop(w);
    let (_, cursor) = read_after(&r, "orders", None).unwrap();
    let bytes = fs::read(seg0(&r)).unwrap();
    fs::write(seg0(&r), &bytes[..SEGMENT_HEADER_LEN]).unwrap();
    assert!(matches!(read_after(&r, "orders", cursor), Err(StoreError::Rewritten { .. })));
}

#[test]
fn a_stream_with_no_segment_has_no_entries_and_no_cursor() {
    let r = root("absent");
    assert_eq!(read_after(&r, "orders", None).unwrap(), (Vec::new(), None));
}
