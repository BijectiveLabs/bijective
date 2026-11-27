// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A program runs, is bounded in time, and what it printed comes back, cut when it is too long.

use std::path::Path;
use std::time::Duration;

use super::{OUTPUT_LIMIT_BYTES, run, truncated};

#[cfg(unix)]
#[test]
fn output_and_exit_code_come_back() {
    let args = vec!["-c".into(), "echo found; echo warned >&2; exit 3".into()];
    let outcome =
        run(Path::new("/bin/sh"), &args, Path::new("/"), Duration::from_secs(10)).unwrap();
    assert_eq!(outcome.exit_code, Some(3));
    assert!(!outcome.succeeded());
    assert!(outcome.output.contains("found") && outcome.output.contains("warned"));
}

/// A stuck program is ended at its bound and the agent is told, instead of waiting forever.
#[cfg(unix)]
#[test]
fn a_program_past_its_bound_is_ended() {
    let args = vec!["-c".into(), "echo started; sleep 30".into()];
    let outcome = run(Path::new("/bin/sh"), &args, Path::new("/"), Duration::from_secs(1)).unwrap();
    assert_eq!(outcome.exit_code, None);
    assert!(outcome.output.contains("started"));
    assert!(outcome.output.contains("its bound"));
}

#[test]
fn a_program_that_cannot_start_is_an_error() {
    let missing = Path::new("/nonexistent/bijective-nothing");
    assert!(run(missing, &[], Path::new("/"), Duration::from_secs(1)).is_err());
}

#[test]
fn long_output_is_cut_and_the_cut_is_said() {
    let text = truncated("x".repeat(OUTPUT_LIMIT_BYTES + 10));
    assert!(text.starts_with(&"x".repeat(OUTPUT_LIMIT_BYTES)));
    assert!(text.contains("10 more bytes not shown"));
}

/// The cut never splits a character, so the text stays valid UTF-8.
#[test]
fn the_cut_falls_on_a_character_boundary() {
    let text = truncated(format!("{}{}", "x".repeat(OUTPUT_LIMIT_BYTES - 1), "é".repeat(4)));
    assert!(text.contains("more bytes not shown"));
}
