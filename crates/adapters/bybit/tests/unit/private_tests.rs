// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The private stream's frames, against the venue's documented examples.

use super::*;
use crate::evidence::Execution;
use crate::sign::{realtime_payload, sign_realtime};

/// Every symbol the fixtures name, at one pair. A symbol left out would read at the
/// outside-catalogue scale (decision 0007).
fn at(price: u8, quantity: u8) -> bijective_venue::profile::InstrumentScales {
    let pair = bijective_venue::profile::Scales {
        price: bijective_venue::normalise::Scale::new(price).unwrap(),
        quantity: bijective_venue::normalise::Scale::new(quantity).unwrap(),
    };
    bijective_venue::profile::InstrumentScales::uniform_over(
        [
            "AAAUSDT",
            "BTCUSDT",
            "ETHUSDT",
            "ROSEUSDT",
            "BTCUSD",
            "ETHPERP",
            "ETH-30DEC22-1400-C",
            "A",
            "S",
        ],
        pair,
    )
}

const STREAM_EXECUTION: &str = include_str!("../../fixtures/documented/stream_execution.json");
const STREAM_ORDER: &str = include_str!("../../fixtures/documented/stream_order.json");

fn scales() -> bijective_venue::profile::InstrumentScales {
    at(1, 3)
}

#[test]
fn the_authentication_frame_signs_get_realtime_and_the_expiry() {
    assert_eq!(realtime_payload(1_662_350_400_000), "GET/realtime1662350400000");
    let frame: serde_json::Value =
        serde_json::from_str(&auth_frame("key", b"secret", 1_662_350_400_000)).unwrap();
    assert_eq!(frame["op"], "auth");
    assert_eq!(frame["args"][0], "key");
    assert_eq!(frame["args"][1], 1_662_350_400_000u64);
    assert_eq!(frame["args"][2], sign_realtime(b"secret", 1_662_350_400_000).as_str());
    assert_eq!(frame["args"][2].as_str().unwrap().len(), 64);
}

#[test]
fn the_subscription_names_the_linear_order_and_execution_topics() {
    let frame: serde_json::Value = serde_json::from_str(&subscribe_frame()).unwrap();
    assert_eq!(frame["args"], serde_json::json!(["order.linear", "execution.linear"]));
}

#[test]
fn the_documented_execution_frame_yields_its_fill() {
    match parse(STREAM_EXECUTION, &scales(), 3).unwrap() {
        PrivateMessage::Executions(rows) => {
            assert_eq!(rows.len(), 1);
            assert!(
                matches!(&rows[0], Ok(Execution::Fill(r)) if r.fill.fill_id == "0ab1bdf7-4219-438b-b30a-32ec863018f7")
            );
        }
        other => panic!("{other:?}"),
    }
}

/// The documented order frame is an option order: refused by row, not dropped by frame.
#[test]
fn the_documented_order_frame_is_judged_row_by_row() {
    match parse(STREAM_ORDER, &scales(), 3).unwrap() {
        PrivateMessage::Orders(rows) => assert_eq!(rows[0].as_ref().unwrap_err().field, "category"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn control_frames_are_read_as_what_they_are() {
    let auth = r#"{"success":true,"ret_msg":"","op":"auth","conn_id":"cejreaspqfh3sjdnldmg-p"}"#;
    assert_eq!(
        parse(auth, &scales(), 0).unwrap(),
        PrivateMessage::Authenticated { success: true, detail: String::new() }
    );
    let pong = r#"{"req_id":"test","op":"pong","args":["1675418560633"],"conn_id":"cfcb4ocsvfriu23r3er0-1b"}"#;
    assert_eq!(parse(pong, &scales(), 0).unwrap(), PrivateMessage::Pong);
    let other = r#"{"topic":"wallet","data":[]}"#;
    assert_eq!(
        parse(other, &scales(), 0).unwrap(),
        PrivateMessage::Unrecognised { topic: Some("wallet".into()) }
    );
    assert!(parse("not json", &scales(), 0).is_err());
}

/// Pushed fills are kept for the evidence reads within the retention bound, a refused row adds
/// nothing, and past the count bound the oldest go first.
#[test]
fn pushed_fills_are_kept_within_their_bounds() {
    let PrivateMessage::Executions(rows) = parse(STREAM_EXECUTION, &scales(), 100).unwrap() else {
        panic!("not an execution frame")
    };
    let mut held = Vec::new();
    retain_pushed(&mut held, &rows, 100);
    assert_eq!(held.len(), 1);
    let refused = vec![Err(RowRefused { field: "execId", detail: "absent".into() })];
    retain_pushed(&mut held, &refused, 100);
    assert_eq!(held.len(), 1, "a refused row adds nothing");
    retain_pushed(&mut held, &[], 100 + PUSHED_KEPT_FOR_NS);
    assert!(held.is_empty(), "a fill older than the bound is dropped");
    for _ in 0..=PUSHED_KEPT_AT_MOST {
        retain_pushed(&mut held, &rows, 100);
    }
    assert_eq!(held.len(), PUSHED_KEPT_AT_MOST);
}
