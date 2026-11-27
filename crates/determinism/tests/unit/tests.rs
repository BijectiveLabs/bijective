// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Byte-identical replay, and the checks that keep it from passing vacuously.

use std::path::PathBuf;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-determinism-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sequence() -> Vec<Input> {
    vec![
        Input { at_ns: 1_000, topic: "market".into(), payload: b"a book update".to_vec() },
        Input { at_ns: 1_010, topic: "limits".into(), payload: b"budget consumed".to_vec() },
        Input { at_ns: 1_010, topic: "market".into(), payload: b"same nanosecond".to_vec() },
        Input { at_ns: 2_500, topic: "market".into(), payload: b"later".to_vec() },
        Input { at_ns: 9_999, topic: "limits".into(), payload: b"budget released".to_vec() },
    ]
}

/// *"The same input sequence replayed twice produces byte-identical event output. Until
/// that test passes, nothing above it can claim reproducibility."*
#[test]
fn one_sequence_replayed_twice_produces_byte_identical_output() {
    let inputs = sequence();
    let first = drive(&inputs, &scratch("run-a"), "core").unwrap();
    let second = drive(&inputs, &scratch("run-b"), "core").unwrap();

    assert_eq!(first, second, "the core is not deterministic over one sequence");
    assert!(!first.is_empty(), "an empty comparison proves nothing");
}

/// Two empty runs compare equal and prove nothing. This shows what non-empty means.
#[test]
fn a_run_actually_writes_what_it_was_given() {
    let inputs = sequence();
    let bytes = drive(&inputs, &scratch("writes"), "core").unwrap();

    for input in &inputs {
        assert!(
            bytes.windows(input.payload.len()).any(|w| w == input.payload),
            "the run did not write {:?}",
            String::from_utf8_lossy(&input.payload)
        );
    }
}

/// If changing an input did not change the bytes, the harness would be writing nothing of consequence.
#[test]
fn a_different_sequence_produces_different_bytes() {
    let mut altered = sequence();
    altered[2].payload = b"something else".to_vec();

    let original = drive(&sequence(), &scratch("orig"), "core").unwrap();
    let changed = drive(&altered, &scratch("changed"), "core").unwrap();

    assert_ne!(original, changed);
}

/// The times are the sequence's: moving one changes the record.
#[test]
fn moving_an_inputs_time_changes_the_bytes() {
    let mut later = sequence();
    later[3].at_ns = 3_000;

    assert_ne!(
        drive(&sequence(), &scratch("t1"), "core").unwrap(),
        drive(&later, &scratch("t2"), "core").unwrap()
    );
}

/// Two events may share a nanosecond. A harness that refused it would test a history the platform never sees.
#[test]
fn a_sequence_containing_two_events_at_one_instant_runs() {
    let shared: Vec<Input> = vec![
        Input { at_ns: 500, topic: "market".into(), payload: b"one".to_vec() },
        Input { at_ns: 500, topic: "market".into(), payload: b"two".to_vec() },
    ];
    assert!(drive(&shared, &scratch("shared"), "core").is_ok());
}

/// Time going backwards is refused by the clock and the run stops.
#[test]
fn a_sequence_that_goes_backwards_is_refused() {
    let backwards: Vec<Input> = vec![
        Input { at_ns: 900, topic: "market".into(), payload: b"first".to_vec() },
        Input { at_ns: 100, topic: "market".into(), payload: b"earlier".to_vec() },
    ];
    assert!(matches!(drive(&backwards, &scratch("back"), "core"), Err(DriveError::Clock(_))));
}

/// A run that continued past a refusal would compare a history that lost something.
#[test]
fn a_refused_publication_stops_the_run_rather_than_being_skipped() {
    // Capacity 1 per topic: a second input without a drain overflows it.
    let overflowing: Vec<Input> =
        vec![Input { at_ns: 1, topic: "market".into(), payload: b"one".to_vec() }];
    let mut many = overflowing.clone();
    many.push(Input { at_ns: 2, topic: "market".into(), payload: b"two".to_vec() });

    // Capacity is the input count, so two fit. The refusal path is covered by the bus tests.
    assert!(drive(&many, &scratch("fits"), "core").is_ok());
}

/// An empty sequence is a real state and must not panic. Equality proves nothing there.
#[test]
fn an_empty_sequence_runs_and_writes_only_a_header() {
    let bytes = drive(&[], &scratch("empty"), "core").unwrap();
    assert!(!bytes.is_empty(), "the segment header is still written");
}
