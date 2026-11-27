// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The performance format reads back exactly what it writes, and refuses what it did not write.

use super::*;

pub(crate) fn sample() -> Performance {
    Performance {
        desk: "alpha".into(),
        strategy: Some("trend".into()),
        venue: "venue-a".into(),
        instrument: "AAA".into(),
        identity: Some("venue-a.AAA".into()),
        quote_currency: Some("usd".into()),
        fills: 3,
        wins: 1,
        losses: 0,
        position: "0.50".into(),
        traded_notional: "301.25".into(),
        realized: "1.20".into(),
        unrealized: Some("-0.05".into()),
        fees: "0.15".into(),
        fees_excluded: vec!["bnb".into()],
        net: Some("1.00".into()),
        peak_net: Some("1.10".into()),
        drawdown: Some("0.10".into()),
        arrival_cost: None,
        arrival_cost_bps: None,
        markouts: vec![Markout { horizon_s: 1, bps: Some("-0.42".into()), fills: 3 }],
        source: "control".into(),
        through: 41,
        at_ns: 1_800_000_000_000_000_000,
    }
}

#[test]
fn an_entry_reads_back_as_written() {
    let written = sample();
    assert_eq!(decode(&encode(&written)), Ok(written));
}

#[test]
fn a_figure_that_is_not_an_exact_decimal_is_refused() {
    let mut value: serde_json::Value = serde_json::from_slice(&encode(&sample())).expect("json");
    value["net"] = serde_json::json!(1.0);
    assert_eq!(decode(value.to_string().as_bytes()), Err(Refused("net")));
    value["net"] = serde_json::json!("1e3");
    assert_eq!(decode(value.to_string().as_bytes()), Err(Refused("net")));
}

#[test]
fn another_format_or_a_missing_field_is_refused() {
    let mut value: serde_json::Value = serde_json::from_slice(&encode(&sample())).expect("json");
    value["format"] = serde_json::json!(2);
    assert_eq!(decode(value.to_string().as_bytes()), Err(Refused("format")));
    let mut value: serde_json::Value = serde_json::from_slice(&encode(&sample())).expect("json");
    value.as_object_mut().expect("object").remove("realized");
    assert_eq!(decode(value.to_string().as_bytes()), Err(Refused("realized")));
}

#[test]
fn each_mode_has_its_own_stream() {
    assert_eq!(stream_of("live"), "performance");
    assert_eq!(stream_of("paper"), "performance-paper");
}
