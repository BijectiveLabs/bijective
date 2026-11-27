// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Replays recorded traffic through the parser and the feed. A mock encodes an assumption about
//! the venue. A recording is what the venue sent.
//!
//! The recording is the venue's always-on test stream (`v2/test`, symbol `FAKEPACA`), taken on
//! 2026-11-17 with `examples/market_probe.rs`: every frame unedited, the welcome, authentication
//! and subscription answers included.

use bijective_venue::contract::MarketFeed;
use bijective_venue::contract::conformance::{RecordedFrame, check_feed};
use bijective_venue::normalise::Scale;
use bijective_venue::profile::{CapabilityDeclaration, VenueProfile};
use bijective_venue_alpaca::Feed;
use bijective_venue_alpaca::market::message::{MarketMessage, parse};

const FIXTURE: &str = include_str!("../fixtures/recorded/test_stream_2026-11-17.jsonl");

fn frames() -> Vec<&'static str> {
    FIXTURE.lines().filter(|l| !l.trim().is_empty()).collect()
}

fn scales() -> (Scale, Scale) {
    (Scale::new(4).expect("price scale"), Scale::new(0).expect("quantity scale"))
}

/// No frame may be refused. A parser that drops what it does not understand passes any fixture.
#[test]
fn every_recorded_frame_is_understood() {
    let (price, quantity) = scales();
    let (mut quotes, mut trades, mut control) = (0, 0, 0);
    for (n, frame) in frames().iter().enumerate() {
        let messages = parse(frame.as_bytes(), price, quantity)
            .unwrap_or_else(|e| panic!("frame {n} was refused by the parser: {e}"));
        for message in messages {
            match message {
                MarketMessage::Quote { .. } => quotes += 1,
                MarketMessage::Trade { .. } => trades += 1,
                MarketMessage::Welcome
                | MarketMessage::Authenticated
                | MarketMessage::Subscribed => {
                    control += 1;
                }
                other => panic!("frame {n} carried {other:?}"),
            }
        }
    }
    assert_eq!(control, 3, "welcome, authentication and subscription were recorded");
    assert!(quotes >= 3 && trades >= 3, "{quotes} quote(s), {trades} trade(s)");
}

fn declared() -> CapabilityDeclaration {
    let profile = serde_json::json!({
        "format": 1,
        "venue": "the venue under replay",
        "verified_at": null,
        "transports": { "stream": {
            "funding": { "model": "continuous" },
            "data_classes": ["top-of-book", "depth", "trades"],
            "max_staleness_ns": "40000000000"
        }},
        "reduce_only": "placement-precondition",
        "order_types": [],
        "instruments": [],
        "identifier_rules": { "allowed": "abc", "max_len": 36 },
        "transformations": [],
        "support": { "venue": "the venue under replay", "depth": "data-only", "debuggable_to_root_cause": true }
    });
    let profile = VenueProfile::from_json(&profile.to_string()).expect("the replay profile");
    CapabilityDeclaration::from_profile(&profile)
}

/// The recorded run, through the feed the live path uses, held to the record contract.
#[test]
fn every_record_from_the_recorded_run_keeps_the_record_contract() {
    let recorded = frames();
    let frames: Vec<RecordedFrame<'_>> = recorded
        .iter()
        .enumerate()
        .map(|(n, frame)| RecordedFrame {
            received_at_ns: 1_794_905_335_000_000_000 + n as i128,
            frame: frame.as_bytes(),
        })
        .collect();
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    let results = check_feed(&mut feed, &frames, &declared());

    for result in &results {
        assert!(result.is_pass(), "{}: {:?}", result.name, result.outcome);
    }
    assert_eq!(feed.instruments(), 1);
}
