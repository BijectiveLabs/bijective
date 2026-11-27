// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The layout round-trips byte for byte, and refuses a payload that does not fit it.

use super::*;

#[test]
fn the_layout_round_trips_byte_for_byte() {
    let u = BookUpdate {
        instrument: "venue-a.ethusdt".into(),
        seq: bijective_model::Sequence::Numbered(u64::MAX),
        venue_ts_ms: 1_787_469_688_129,
        received_at_ns: None,
        bid_price: -170_141_183_460_469_231_731_687_303_715_884_105_727,
        bid_quantity: 1,
        ask_price: 170_141_183_460_469_231_731_687_303_715_884_105_727,
        ask_quantity: 0,
        bid_depth: u32::MAX,
        ask_depth: 0,
    };

    let first = encode(&u).unwrap();
    let back = decode(&first).unwrap();
    assert_eq!(back, u);
    assert_eq!(encode(&back).unwrap(), first);
}

#[test]
fn a_payload_with_a_tail_is_refused() {
    let u = BookUpdate {
        instrument: "v.s".into(),
        seq: bijective_model::Sequence::NotNumbered,
        venue_ts_ms: 2,
        received_at_ns: None,
        bid_price: 3,
        bid_quantity: 4,
        ask_price: 5,
        ask_quantity: 6,
        bid_depth: 7,
        ask_depth: 8,
    };
    let mut bytes = encode(&u).unwrap();
    bytes.push(0);
    assert!(decode(&bytes).is_err());
}
