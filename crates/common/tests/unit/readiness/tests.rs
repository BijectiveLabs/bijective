// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The surface is only useful if it cannot say yes by accident.

use std::path::PathBuf;

use super::*;

const T0: i128 = 1_700_000_000_000_000_000;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-readiness-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("readiness.json")
}

#[test]
fn a_reported_state_reads_back() {
    let p = scratch("roundtrip");
    let s = Surface::new(&p, "data-engine");
    s.report(Health::Ready, "ingesting", T0).unwrap();

    let doc = read_at(&p, T0).unwrap();
    assert_eq!(doc.health, Health::Ready);
    assert_eq!(doc.program, "data-engine");
    assert_eq!(doc.stamped_at_ns(), Some(T0));
}

/// The deployment kit: running is not ready. The distinction is the component's reason for existing.
#[test]
fn a_program_that_is_replaying_is_not_ready() {
    let p = scratch("replaying");
    Surface::new(&p, "control-engine").report(Health::Replaying, "limit state at 40%", T0).unwrap();

    assert!(!is_ready(&p, T0));
    assert_eq!(read_at(&p, T0).unwrap().health, Health::Replaying);
}

/// A correctly halted Control Engine started properly. The trading state must not read as a failed start.
#[test]
fn a_started_program_that_is_not_trading_is_still_ready() {
    let p = scratch("halted");
    Surface::new(&p, "control-engine")
        .report(Health::Ready, "limit state replayed and verified; trading halted", T0)
        .unwrap();

    assert!(is_ready(&p, T0));
    assert!(read_at(&p, T0).unwrap().detail.contains("halted"));
}

/// A dead program leaves its last claim behind. Only the reader's clock exposes it.
#[test]
fn a_document_left_behind_by_a_dead_program_goes_stale() {
    let p = scratch("stale");
    Surface::new(&p, "detector").report(Health::Ready, "watching", T0).unwrap();

    assert!(is_ready(&p, T0 + document::MAX_AGE_NS));
    assert!(!is_ready(&p, T0 + document::MAX_AGE_NS + 1));
    assert!(matches!(
        read_at(&p, T0 + document::MAX_AGE_NS + 1),
        Err(ReadinessError::Stale { .. })
    ));
}

#[test]
fn an_absent_document_is_not_ready_and_says_which_way_it_failed() {
    let p = scratch("absent").with_file_name("nothing.json");
    assert!(!is_ready(&p, T0));
    assert_eq!(read_at(&p, T0), Err(ReadinessError::Absent));
}

#[test]
fn a_truncated_document_is_not_ready() {
    let p = scratch("torn");
    std::fs::write(&p, b"{\"version\":1,\"program\":\"data-eng").unwrap();
    assert!(!is_ready(&p, T0));
    assert_eq!(read_at(&p, T0), Err(ReadinessError::Unreadable));
}

/// An unknown version refuses rather than reading the fields it recognises.
#[test]
fn a_document_from_another_build_is_refused_not_interpreted() {
    let p = scratch("version");
    std::fs::write(
        &p,
        br#"{"version":99,"program":"x","health":"ready","detail":"","stamped_at_ns":"1"}"#,
    )
    .unwrap();

    assert!(!is_ready(&p, T0));
    assert!(matches!(read_at(&p, T0), Err(ReadinessError::VersionUnknown { found: 99 })));
}

#[test]
fn a_replacement_never_leaves_a_reader_holding_half_a_document() {
    let p = scratch("atomic");
    let s = Surface::new(&p, "data-engine");
    s.report(Health::Starting, "opening the store", T0).unwrap();

    for i in 0..40 {
        s.report(Health::Ready, format!("tick {i}"), T0 + i).unwrap();
        // Every read between replacements parses. A non-atomic write fails this quickly.
        assert!(read_at(&p, T0 + i).is_ok());
    }
}

#[cfg(unix)]
#[test]
fn the_document_is_readable_without_privilege() {
    use std::os::unix::fs::PermissionsExt;
    let p = scratch("mode");
    Surface::new(&p, "dashboard").report(Health::Ready, "listening", T0).unwrap();

    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "an operator must not need privilege to ask if the node is up");
}

/// Two instances of one program share a run directory, and must not unwrite each other.
/// One Data Engine runs per venue, so instances share a run directory. A temp file named for the program alone made them collide.
#[test]
fn two_instances_of_one_program_can_report_side_by_side() {
    let dir = std::env::temp_dir().join(format!("bl-readiness-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");

    let first = Surface::new(dir.join("first.json"), "data-engine");
    let second = Surface::new(dir.join("second.json"), "data-engine");

    // Interleaved: the failure is a temp file one takes from the other between create and rename.
    for round in 0..32 {
        first.report(Health::Starting, "one", round).expect("the first instance could not report");
        second
            .report(Health::Starting, "two", round)
            .expect("the second instance could not report");
    }

    let one = read_at(&dir.join("first.json"), 100).expect("the first document is unreadable");
    let two = read_at(&dir.join("second.json"), 100).expect("the second document is unreadable");
    assert_eq!(one.detail, "one");
    assert_eq!(two.detail, "two", "one instance overwrote the other's document");

    let _ = std::fs::remove_dir_all(&dir);
}
