// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Gates for the file-backed store.
//! Crashes are produced as a crash does, by truncating or extending a real file. A mock would agree with our beliefs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::digest::Digest;
use super::format::*;
use super::store::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A private directory per test, cleared on the way in so a failed test leaves its files for diagnosis.
fn root(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bijective-event-store-{}-{}-{}",
        std::process::id(),
        name,
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn seg0(root: &Path, stream: &str) -> PathBuf {
    root.join(stream).join("000000000000.seg")
}

#[test]
fn append_assigns_gap_free_positions_and_reads_back() {
    let r = root("append");
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        assert_eq!(w.append(b"one").unwrap(), 0);
        assert_eq!(w.append(b"two").unwrap(), 1);
        assert_eq!(w.append(b"three").unwrap(), 2);
    }

    let got = read_range(&r, "orders", 0, 10).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].payload, b"one".to_vec());
    assert_eq!(got[2].seq, 2);

    // Range is by position, and it is inclusive at both ends.
    let mid = read_range(&r, "orders", 1, 1).unwrap();
    assert_eq!(mid.len(), 1);
    assert_eq!(mid[0].payload, b"two".to_vec());
}

#[test]
fn a_reopened_stream_continues_the_sequence_and_the_chain() {
    let r = root("reopen");
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        w.append(b"one").unwrap();
        w.append(b"two").unwrap();
    }
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        assert_eq!(w.next_sequence(), 2);
        assert_eq!(w.append(b"three").unwrap(), 2);
    }
    assert!(matches!(verify_segment(&seg0(&r, "orders")), Verdict::Clean { entries: 3, .. }));
}

/// Two writers on one stream is refused, not serialised.
/// The process model: single writer per stream. Detecting a duplicate afterwards is not the same guarantee.
#[test]
fn a_second_writer_is_refused() {
    let r = root("lock");
    let _first = StreamWriter::open(&r, "orders").unwrap();
    match StreamWriter::open(&r, "orders") {
        Err(StoreError::StreamLocked { stream }) => assert_eq!(stream, "orders"),
        other => panic!("expected StreamLocked, got {other:?}"),
    }
}

#[test]
fn the_lock_is_released_on_drop_so_a_clean_shutdown_reopens() {
    let r = root("unlock");
    {
        let _w = StreamWriter::open(&r, "orders").unwrap();
    }
    assert!(StreamWriter::open(&r, "orders").is_ok());
}

/// Killed mid-append: the store recovers, seals what was whole, and does not overwrite.
#[test]
fn a_torn_tail_is_sealed_off_rather_than_written_over() {
    let r = root("torn");
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        w.append(b"one").unwrap();
        w.append(b"two").unwrap();
    }

    // Simulate the crash: a partial third entry reaches the disk.
    let path = seg0(&r, "orders");
    let before = fs::metadata(&path).unwrap().len();
    {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&entry_header(2, 999, Digest::genesis())).unwrap();
        f.write_all(b"partial").unwrap();
    }
    let with_tail = fs::metadata(&path).unwrap().len();
    assert!(with_tail > before);

    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        assert_eq!(w.next_sequence(), 2, "the partial entry was never a record");
        assert_eq!(w.append(b"three").unwrap(), 2);
    }

    // The damaged segment is sealed and its bytes are untouched: append-only holds literally.
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        with_tail,
        "the torn segment must not be rewritten"
    );
    let m = read_manifest(&r.join("orders").join("000000000000.manifest"))
        .expect("the torn segment was sealed");
    assert_eq!(m.entries, 2);
    assert_eq!(m.bytes, before, "sealed at the last whole entry");

    // The continuation went to a new segment, and the whole stream still replays in order.
    let all = read_range(&r, "orders", 0, 99).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[2].payload, b"three".to_vec());
}

/// A corrupt segment stops the open. Fail closed.
/// Appending onto an unverified history would extend the chain from a link that may be wrong.
#[test]
fn a_corrupt_segment_refuses_the_open() {
    let r = root("corrupt");
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        w.append(b"one").unwrap();
        w.append(b"two").unwrap();
    }
    let path = seg0(&r, "orders");
    let mut bytes = fs::read(&path).unwrap();
    bytes[SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    match StreamWriter::open(&r, "orders") {
        Err(StoreError::Quarantined { verdict, .. }) => {
            assert!(matches!(verdict, Verdict::Corrupt { .. }));
        }
        other => panic!("expected Quarantined, got {other:?}"),
    }

    // The failed open released the lock, so the next attempt fails for the real reason.
    assert!(matches!(StreamWriter::open(&r, "orders"), Err(StoreError::Quarantined { .. })));
}

/// A quarantined segment is not silently skipped on read either.
#[test]
fn a_corrupt_segment_stops_a_replay_rather_than_leaving_a_hole() {
    let r = root("replay-hole");
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        w.append(b"one").unwrap();
    }
    let path = seg0(&r, "orders");
    let mut bytes = fs::read(&path).unwrap();
    bytes[SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    assert!(matches!(read_range(&r, "orders", 0, 10), Err(StoreError::Quarantined { .. })));
}

/// Sealing makes the segment read-only, and the manifest describes it.
/// A batch written without a sync per entry reads back the same as entries appended one by one,
/// and the chain links across it, so a reopened writer continues it.
#[test]
fn a_batch_synced_once_reads_back_and_the_chain_continues() {
    let r = root("batch");
    {
        let mut w = StreamWriter::open(&r, "frames").unwrap();
        assert_eq!(w.append_unsynced(b"one").unwrap(), 0);
        assert_eq!(w.append_unsynced(b"two").unwrap(), 1);
        w.sync().unwrap();
        assert_eq!(w.append(b"three").unwrap(), 2);
    }
    let got = read_range(&r, "frames", 0, 10).unwrap();
    let payloads: Vec<_> = got.iter().map(|e| e.payload.clone()).collect();
    assert_eq!(payloads, vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);

    let mut w = StreamWriter::open(&r, "frames").unwrap();
    assert_eq!(w.append(b"four").unwrap(), 3, "the reopened writer continues the sequence");
}

/// A manifest describes what is on disk, so a rotation counts entries appended without a sync.
#[test]
fn a_rotation_seals_entries_appended_without_a_sync() {
    let r = root("batch-seal");
    let mut w = StreamWriter::open(&r, "frames").unwrap();
    w.append_unsynced(b"one").unwrap();
    w.append_unsynced(b"two").unwrap();
    let m = w.rotate().unwrap();
    assert_eq!((m.entries, m.first_seq, m.last_seq), (2, 0, 1));
    assert_eq!(read_range(&r, "frames", 0, 1).unwrap().len(), 2);
}

#[test]
fn a_sealed_segment_is_read_only_and_described() {
    let r = root("seal");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    w.append(b"one").unwrap();
    w.append(b"two").unwrap();
    let m = w.rotate().unwrap();

    assert_eq!(m.entries, 2);
    assert_eq!(m.first_seq, 0);
    assert_eq!(m.last_seq, 1);
    assert_eq!(m.format, FORMAT_VERSION);

    let path = seg0(&r, "orders");
    assert!(
        fs::metadata(&path).unwrap().permissions().readonly(),
        "a sealed segment must be incapable of accepting an append, not merely closed to it"
    );
    assert!(
        fs::OpenOptions::new().append(true).open(&path).is_err(),
        "the filesystem refuses the append, not our discipline"
    );
}

#[test]
fn a_manifest_edited_by_hand_does_not_pass() {
    let r = root("manifest-edit");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    w.append(b"one").unwrap();
    w.rotate().unwrap();

    let mpath = r.join("orders").join("000000000000.manifest");
    assert!(read_manifest(&mpath).is_some());

    let text = fs::read_to_string(&mpath).unwrap().replace("entries 1", "entries 9");
    fs::write(&mpath, text).unwrap();
    assert!(read_manifest(&mpath).is_none(), "the body no longer matches its digest");
}

/// Rotation keeps the chain across the boundary: a new segment starts from the last link.
#[test]
fn the_chain_continues_across_a_rotation() {
    let r = root("rotate-chain");
    let mut w = StreamWriter::open(&r, "orders")
        .unwrap()
        .with_max_segment_bytes((SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + 8) as u64);

    for i in 0..6u8 {
        w.append(&[i; 4]).unwrap();
    }
    drop(w);

    let count = fs::read_dir(r.join("orders"))
        .unwrap()
        .filter_map(|e| {
            let n = e.ok()?.file_name().to_string_lossy().to_string();
            n.strip_suffix(".seg").map(|s| s.to_string())
        })
        .count();
    assert!(count > 1, "the bound should have forced a rotation");

    let all = read_range(&r, "orders", 0, 99).unwrap();
    assert_eq!(all.len(), 6);
    for (i, e) in all.iter().enumerate() {
        assert_eq!(e.seq, i as u64, "positions stay gap-free across segments");
    }
}

/// A stream name that would escape the store root is refused.
#[test]
fn a_traversing_stream_name_is_refused() {
    let r = root("names");
    for bad in ["..", "a/b", "", "a b"] {
        assert!(
            matches!(StreamWriter::open(&r, bad), Err(StoreError::BadStreamName)),
            "expected {bad:?} to be refused"
        );
    }
    assert!(StreamWriter::open(&r, "orders_v2-1").is_ok());
}

/// An absent stream reads as empty. An unreadable segment does not.
#[test]
fn absent_is_empty_and_unreadable_is_not() {
    let r = root("absent");
    assert!(read_range(&r, "orders", 0, 10).unwrap().is_empty());

    let dir = r.join("garbage");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("000000000000.seg"), b"not one of ours").unwrap();
    match read_range(&r, "garbage", 0, 10) {
        Err(StoreError::Quarantined { verdict, .. }) => {
            assert!(matches!(verdict, Verdict::Unreadable { .. }))
        }
        other => panic!("expected Unreadable, got {other:?}"),
    }
}

#[test]
fn an_oversized_payload_is_refused_rather_than_written() {
    let r = root("oversize");
    let mut w = StreamWriter::open(&r, "orders").unwrap();
    let big = vec![0u8; MAX_PAYLOAD_LEN as usize + 1];
    assert!(matches!(w.append(&big), Err(StoreError::PayloadTooLarge { .. })));
    assert_eq!(w.next_sequence(), 0, "a refused append consumes no position");
}

/// A reader that wants the present is told where it is, without replaying the past.
#[test]
fn the_end_of_a_stream_is_the_last_position_written() {
    let dir = root("end-of");
    let mut w = StreamWriter::open(&dir, "market").expect("open");
    for i in 0..5u8 {
        w.append(&[i]).expect("append");
    }

    assert_eq!(end_of(&dir, "market").expect("end"), Some(4));
}

/// An empty history and a history that was never read are different answers.
#[test]
fn a_stream_nobody_has_written_has_no_end_rather_than_position_zero() {
    let dir = root("end-of-empty");

    assert_eq!(end_of(&dir, "never-written").expect("end"), None);
}

/// The end survives a rotation, which is where a naive answer reads the wrong segment.
#[test]
fn the_end_is_the_newest_segment_not_the_first() {
    let dir = root("end-of-rotated");
    let mut w = StreamWriter::open(&dir, "market").expect("open").with_max_segment_bytes(64);
    for i in 0..40u8 {
        w.append(&[i; 8]).expect("append");
    }

    let end = end_of(&dir, "market").expect("end");
    assert_eq!(end, Some(39), "the end must come from the newest segment");
}

/// A store this process can write says so, and one it cannot says that.
/// Asked by trying it: the mount and ownership decide, not a permission bit.
#[test]
fn a_store_that_cannot_be_written_is_refused_before_anything_needs_writing() {
    let dir = root("writable");
    assert!(writable(&dir).is_ok(), "a fresh directory should take a write");

    let shut = dir.join("shut");
    fs::create_dir_all(&shut).expect("create");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = fs::metadata(&shut).expect("metadata").permissions();
        mode.set_mode(0o500);
        fs::set_permissions(&shut, mode).expect("chmod");
    }

    // Established first: a process that can write anywhere (root) cannot express not-writable.
    if fs::write(shut.join("probe"), b"").is_ok() {
        return;
    }

    assert!(
        writable(&shut).is_err(),
        "a directory this process cannot write was reported as writable"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = fs::metadata(&shut).expect("metadata").permissions();
        mode.set_mode(0o700);
        let _ = fs::set_permissions(&shut, mode);
    }
}

/// The newest entries, in order, read from the newest segments only: a reader that wants the last
/// hundred book updates must not pay for a day of them.
#[test]
fn a_tail_is_the_newest_entries_in_order_across_segments() {
    let r = root("tail");
    let mut w = StreamWriter::open(&r, "market")
        .unwrap()
        .with_max_segment_bytes((SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + 8) as u64);
    for i in 0..6u8 {
        w.append(&[i; 4]).unwrap();
    }
    drop(w);

    let tail = read_tail(&r, "market", 3).unwrap();
    assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    assert_eq!(tail[0].payload, vec![3; 4]);
    assert_eq!(read_tail(&r, "market", 99).unwrap().len(), 6, "a short stream is read whole");
    assert!(read_tail(&r, "market", 0).unwrap().is_empty());
    assert!(read_tail(&r, "never-written", 5).unwrap().is_empty());
}

/// A damaged segment in the part of the tail being read is an error, never a shorter tail.
#[test]
fn a_corrupt_segment_in_the_tail_is_an_error_not_a_shorter_tail() {
    let r = root("tail-corrupt");
    let mut w = StreamWriter::open(&r, "market").unwrap();
    for i in 0..3u8 {
        w.append(&[i; 4]).unwrap();
    }
    drop(w);
    let path = seg0(&r, "market");
    let mut bytes = fs::read(&path).unwrap();
    bytes[SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(read_tail(&r, "market", 2), Err(StoreError::Quarantined { .. })));
}

/// A sealed segment cut short at an entry boundary chains cleanly. Its manifest says what was sealed.
#[test]
fn a_sealed_segment_cut_short_is_refused_by_its_manifest() {
    let r = root("cut-short");
    {
        let mut w = StreamWriter::open(&r, "orders").unwrap();
        w.append(b"one").unwrap();
        w.append(b"two").unwrap();
        w.rotate().unwrap();
        w.append(b"three").unwrap();
    }
    let seg = seg0(&r, "orders");
    let whole = fs::read(&seg).unwrap();
    let cut = whole.len() - ENTRY_HEADER_LEN - b"two".len();
    let mut perms = fs::metadata(&seg).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&seg, perms).unwrap();
    fs::write(&seg, &whole[..cut]).unwrap();

    match verify_segment(&seg) {
        Verdict::Corrupt { reason: CorruptReason::ManifestMismatch, at_offset } => {
            assert_eq!(at_offset, cut as u64);
        }
        other => panic!("expected Corrupt against the manifest, got {other:?}"),
    }
    assert!(matches!(read_range(&r, "orders", 0, 10), Err(StoreError::Quarantined { .. })));
}
