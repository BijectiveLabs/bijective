// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Replays recorded live traffic through the parser and the feed.
//! A mock encodes an assumption about the venue. A recording is what the venue sent.
//!
//! The recording is 75 seconds of `l2Book` for BTC and ETH on mainnet, taken with the
//! `record_frames` example on 2026-11-04 and left unedited.

use bijective_venue::contract::MarketFeed;
use bijective_venue::contract::conformance::{RecordedFrame, check_feed};
use bijective_venue::normalise::Scale;
use bijective_venue::profile::{CapabilityDeclaration, VenueProfile};
use bijective_venue_hyperliquid::Feed;
use bijective_venue_hyperliquid::message::{VenueMessage, parse};
use serde::Deserialize;

const FIXTURE: &str = include_str!("../fixtures/hyperliquid_l2book.jsonl");

/// Wide enough for every coin in the recording: the venue gives each its own size decimals.
const SCALE: u8 = 6;

#[derive(Deserialize)]
struct Recorded {
    at_ns: String,
    frame: String,
}

fn recorded() -> Vec<(i128, String)> {
    FIXTURE
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let rec: Recorded = serde_json::from_str(l).expect("fixture line is readable");
            (rec.at_ns.parse().expect("arrival time"), rec.frame)
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    books: usize,
    acks: usize,
    pongs: usize,
    ignored: usize,
    refused: usize,
}

fn tally() -> Tally {
    let scale = Scale::new(SCALE).expect("scale");
    let mut t = Tally::default();
    for (n, (_, frame)) in recorded().iter().enumerate() {
        let message = parse(frame.as_bytes(), scale, scale)
            .unwrap_or_else(|e| panic!("frame {n} was refused by the parser: {e}"));
        match message {
            VenueMessage::Book { bids, asks, .. } => {
                assert!(!bids.is_empty() && !asks.is_empty(), "frame {n} carries an empty side");
                t.books += 1;
            }
            VenueMessage::SubscriptionAccepted => t.acks += 1,
            VenueMessage::Pong => t.pongs += 1,
            VenueMessage::Ignored => t.ignored += 1,
            VenueMessage::Trades { .. } | VenueMessage::AssetContext { .. } => {
                panic!("frame {n}: the recorded book traffic holds no trades or context")
            }
            VenueMessage::Refused { .. } => t.refused += 1,
        }
    }
    t
}

/// No frame may be refused or quietly set aside. A parser that drops what it does not understand
/// passes any fixture.
#[test]
fn every_recorded_frame_is_understood() {
    let t = tally();

    assert_eq!(t.refused, 0, "the venue refused something during the recording");
    assert_eq!(t.ignored, 0, "a recorded frame was not recognised");
    assert!(t.books >= 20, "the recording carries a real run, found {} books", t.books);
}

/// The acknowledgement and the ping cycle are part of the venue's behaviour, not only its data.
#[test]
fn the_recording_covers_the_choreography_and_not_only_the_data() {
    let t = tally();

    assert_eq!(t.acks, 2, "one acknowledgement per subscribed coin");
    assert!(t.pongs >= 1, "no pong was recorded, so the ping cycle is unproven");
}

/// A declaration serving depth and nothing else, so an emitted class the venue was not declared
/// for fails the replay.
fn depth_only() -> CapabilityDeclaration {
    let profile = serde_json::json!({
        "format": 1,
        "venue": "the venue under replay",
        "verified_at": null,
        "transports": { "stream": {
            "funding": { "model": "published", "interval_ns": "28800000000000" },
            "data_classes": ["depth"],
            "max_staleness_ns": "40000000000"
        }},
        "reduce_only": "fill-time-clamp",
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
    let recorded = recorded();
    let frames: Vec<RecordedFrame<'_>> = recorded
        .iter()
        .map(|(at, frame)| RecordedFrame { received_at_ns: *at, frame: frame.as_bytes() })
        .collect();
    let scale = Scale::new(SCALE).expect("scale");
    let mut feed = Feed::new(scale, scale);

    let results = check_feed(&mut feed, &frames, &depth_only());

    for result in &results {
        assert!(result.is_pass(), "{}: {:?}", result.name, result.outcome);
    }
    assert_eq!(feed.instruments(), 2, "one book per coin in the recording");
}
