// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The detections format reads back exactly what it writes and refuses anything else.

use super::*;

fn raised() -> Detection {
    Detection {
        detector: "fill-lag".into(),
        subject: "venue:venue-a".into(),
        level: Raised::Attention,
        detail: "fills arrive 12.5 s after the venue's time".into(),
        measured: Some("12.500".into()),
        threshold: Some("10".into()),
        source: "control".into(),
        evidence: vec![9, 31],
        at_ns: 1_795_445_940_668_249_097,
    }
}

#[test]
fn a_detection_reads_back_as_written() {
    let detection = raised();
    assert_eq!(decode(&encode(&detection)), Ok(detection));
    let cleared = Detection { level: Raised::Cleared, measured: None, threshold: None, ..raised() };
    assert_eq!(decode(&encode(&cleared)), Ok(cleared));
}

#[test]
fn a_time_or_position_too_large_for_a_float_survives() {
    let text = String::from_utf8(encode(&raised())).expect("utf-8");
    assert!(text.contains("\"at_ns\":\"1795445940668249097\""), "{text}");
    assert!(text.contains("\"evidence\":[\"9\",\"31\"]"), "{text}");
}

#[test]
fn a_malformed_entry_is_refused_naming_its_field() {
    let with = |field: &str, value: Value| {
        let mut object: Value = serde_json::from_slice(&encode(&raised())).expect("json");
        object[field] = value;
        decode(object.to_string().as_bytes())
    };
    assert_eq!(with("format", json!(2)), Err(Refused("format")));
    assert_eq!(with("level", json!("panic")), Err(Refused("level")));
    assert_eq!(with("subject", json!("")), Err(Refused("subject")));
    assert_eq!(with("measured", json!(12.5)), Err(Refused("measured")), "a float is not exact");
    assert_eq!(with("measured", json!("1.2.3")), Err(Refused("measured")));
    assert_eq!(with("at_ns", json!(17)), Err(Refused("at_ns")));
    assert_eq!(with("evidence", json!(["x"])), Err(Refused("evidence")));
    assert_eq!(decode(b"[1]"), Err(Refused("not an object")));
}
