// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::format::{ENTRY_HEADER_LEN, SEGMENT_HEADER_LEN};
use crate::store::{StreamWriter, end_of, read_range};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn root(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("bijective-event-store-retention-{}-{name}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Ten entries of ten bytes, two to a segment: segments hold 0-1, 2-3, 4-5, 6-7 and the open 8-9.
fn ten_entries(r: &Path) -> StreamWriter {
    let two = (SEGMENT_HEADER_LEN + 2 * (ENTRY_HEADER_LEN + 10)) as u64;
    let mut w = StreamWriter::open(r, "frames").unwrap().with_max_segment_bytes(two);
    for n in 0..10u8 {
        w.append(&[n; 10]).unwrap();
    }
    w
}

fn segments(r: &Path) -> usize {
    fs::read_dir(r.join("frames"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".seg"))
        .count()
}

#[test]
fn a_prune_removes_the_prefix_before_the_position_and_reads_refuse_below_it() {
    let r = root("prefix");
    let _w = ten_entries(&r);
    assert_eq!(segments(&r), 5);

    let pruned = prune_before(&r, "frames", 5).unwrap();
    assert_eq!((pruned.segments, pruned.entries, pruned.horizon), (2, 4, Some(4)));
    assert_eq!(segments(&r), 3);

    let kept = read_range(&r, "frames", 4, 9).unwrap();
    assert_eq!(kept.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![4, 5, 6, 7, 8, 9]);
    assert!(matches!(read_range(&r, "frames", 0, 9), Err(StoreError::Pruned { horizon: 4, .. })));
    assert_eq!(end_of(&r, "frames").unwrap(), Some(9));
}

#[test]
fn the_newest_segment_is_never_removed_and_the_writer_continues() {
    let r = root("newest");
    let mut w = ten_entries(&r);

    let pruned = prune_before(&r, "frames", u64::MAX).unwrap();
    assert_eq!((pruned.segments, pruned.horizon), (4, Some(8)));
    assert_eq!(segments(&r), 1);

    assert_eq!(w.append(&[10; 10]).unwrap(), 10);
    drop(w);
    let reopened = StreamWriter::open(&r, "frames").unwrap();
    assert_eq!(reopened.next_sequence(), 11, "the chain and the count survive the prune");
    assert_eq!(read_range(&r, "frames", 8, 10).unwrap().len(), 3);
}

#[test]
fn a_position_inside_a_segment_keeps_that_segment() {
    let r = root("inside");
    let _w = ten_entries(&r);
    // Position 3 is the last of segment 2-3. Nothing at or after it may go.
    let pruned = prune_before(&r, "frames", 3).unwrap();
    assert_eq!((pruned.segments, pruned.horizon), (1, Some(2)));
    assert_eq!(read_range(&r, "frames", 2, 3).unwrap().len(), 2);
}

#[test]
fn a_prune_with_nothing_before_the_position_changes_nothing() {
    let r = root("nothing");
    let _w = ten_entries(&r);
    assert_eq!(prune_before(&r, "frames", 1).unwrap(), Pruned::default());
    assert_eq!(horizon(&r, "frames").unwrap(), None);
    assert_eq!(read_range(&r, "frames", 0, 9).unwrap().len(), 10);
}

#[test]
fn a_second_prune_moves_the_horizon_forward_and_reports_only_what_it_removed() {
    let r = root("twice");
    let _w = ten_entries(&r);
    prune_before(&r, "frames", 3).unwrap();
    let second = prune_before(&r, "frames", 7).unwrap();
    assert_eq!((second.segments, second.entries, second.horizon), (2, 4, Some(6)));
    assert_eq!(horizon(&r, "frames").unwrap().unwrap().first_seq, 6);
}

#[test]
fn an_edited_horizon_is_refused_rather_than_read_past() {
    let r = root("edited");
    let _w = ten_entries(&r);
    prune_before(&r, "frames", 5).unwrap();
    let path = r.join("frames").join("horizon");
    let text = fs::read_to_string(&path).unwrap().replace("first_seq 4", "first_seq 0");
    fs::write(&path, text).unwrap();

    assert!(matches!(horizon(&r, "frames"), Err(StoreError::HorizonUnreadable { .. })));
    assert!(matches!(read_range(&r, "frames", 4, 9), Err(StoreError::HorizonUnreadable { .. })));
}

#[test]
fn a_segment_an_interrupted_prune_left_below_the_horizon_is_not_read() {
    let r = root("interrupted");
    let _w = ten_entries(&r);
    let dir = r.join("frames");
    let first = dir.join("000000000000.seg");
    let saved = fs::read(&first).unwrap();
    prune_before(&r, "frames", 5).unwrap();
    // As if the process died after the horizon was written and before this segment was removed.
    fs::write(&first, saved).unwrap();

    let kept = read_range(&r, "frames", 4, 9).unwrap();
    assert_eq!(kept.first().map(|e| e.seq), Some(4));
    assert_eq!(kept.len(), 6);
}

#[test]
fn the_retained_segments_extend_the_link_the_horizon_records() {
    let r = root("link");
    let _w = ten_entries(&r);
    let before = read_range(&r, "frames", 0, 9).unwrap();
    prune_before(&r, "frames", 5).unwrap();
    let h = horizon(&r, "frames").unwrap().unwrap();
    assert_eq!(h.base_link, before[3].link, "the horizon names the link entry 3 ended on");
}
