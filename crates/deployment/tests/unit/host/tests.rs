// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A sweep must tell an empty store from a damaged one, and must not miss a nested segment.

use std::path::PathBuf;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-host-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_empty_store_is_healthy_not_damaged() {
    let root = scratch("empty");
    let found = sweep(&root);

    assert_eq!(found, Sweep::default());
    assert!(found.is_healthy(), "a fresh node has appended nothing and is not broken");
}

#[test]
fn a_root_that_does_not_exist_is_reported_rather_than_panicking() {
    let found = sweep(&PathBuf::from("/nonexistent/store/root"));
    assert_eq!(found.total(), 0);
}

/// The store nests by stream. A one-level walk once cost a fail-open gate.
#[test]
fn a_segment_nested_under_a_stream_is_found() {
    let root = scratch("nested");
    std::fs::create_dir_all(root.join("market")).unwrap();
    std::fs::write(root.join("market/000000000000.seg"), b"not a real segment").unwrap();

    let found = sweep(&root);
    assert_eq!(found.total(), 1, "the nested segment was not reached");
    assert_eq!(found.damaged, 1, "unreadable bytes are damage, not absence");
    assert!(!found.is_healthy());
}

#[test]
fn a_clean_segment_with_a_torn_tail_is_clean_and_not_complete() {
    use bijective_event_store::store::StreamWriter;

    let root = scratch("torn");
    {
        let mut w = StreamWriter::open(&root, "market").unwrap();
        w.append(b"a whole entry").unwrap();
        w.append(b"another whole entry").unwrap();
    }
    let segment = std::fs::read_dir(root.join("market"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "seg"))
        .expect("a segment was written");

    let mut bytes = std::fs::read(&segment).unwrap();
    bytes.extend_from_slice(b"half an entry");
    std::fs::write(&segment, bytes).unwrap();

    let found = sweep(&root);
    assert!(found.is_healthy(), "a torn tail is the crash signature, not damage");
    assert!(!found.is_complete(), "and it must not read as a complete copy");
    assert_eq!(found.with_torn_tail, 1);
    assert!(found.torn_tail_bytes > 0);
}

#[test]
fn a_file_that_is_not_a_segment_is_not_counted() {
    let root = scratch("other-files");
    std::fs::write(root.join("manifest.json"), b"{}").unwrap();
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();

    assert_eq!(sweep(&root).total(), 0);
}

#[test]
fn one_damaged_segment_makes_the_sweep_unhealthy() {
    let root = scratch("mixed");
    std::fs::create_dir_all(root.join("limits")).unwrap();
    std::fs::write(root.join("limits/000000000000.seg"), b"rubbish").unwrap();
    std::fs::write(root.join("limits/000000000001.seg"), b"rubbish too").unwrap();

    let found = sweep(&root);
    assert_eq!(found.damaged, 2);
    assert!(!found.is_healthy());
}
