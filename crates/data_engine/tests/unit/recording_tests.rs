// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reading a recording line: the arrival time is part of the frame's record, never invented.

use crate::replay::extract_frame;

#[test]
fn a_recorded_line_yields_its_arrival_time_and_its_frame() {
    let line = r#"{"at_ns":"1787469688200000000","frame":"{\"op\":\"pong\"}"}"#;
    assert_eq!(
        extract_frame(line),
        Some((1_787_469_688_200_000_000, r#"{"op":"pong"}"#.to_string()))
    );
}

/// A replay that defaulted the arrival time would put every record at one instant.
#[test]
fn a_line_without_an_arrival_time_is_refused() {
    assert_eq!(extract_frame(r#"{"frame":"{}"}"#), None);
    assert_eq!(extract_frame(r#"{"at_ns":17,"frame":"{}"}"#), None, "a JSON number is a double");
    assert_eq!(extract_frame(r#"{"at_ns":"soon","frame":"{}"}"#), None);
}
