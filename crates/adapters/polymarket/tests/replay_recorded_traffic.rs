// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Replays recorded live traffic through the parser and the feed.
//! A mock encodes an assumption about the venue. A recording is what the venue sent.
//!
//! The recording is the market stream for one market's two outcomes, taken on 2026-11-15: the
//! opening frame kept to that market's two books (it carried the books of every subscribed
//! outcome), then every later frame about that market, unedited, with the `PONG` answers.

use bijective_venue::contract::MarketFeed;
use bijective_venue::contract::conformance::{RecordedFrame, check_feed};
use bijective_venue::normalise::Scale;
use bijective_venue::profile::{CapabilityDeclaration, VenueProfile};
use bijective_venue_polymarket::Feed;
use bijective_venue_polymarket::message::{VenueEvent, parse};

const FIXTURE: &str = include_str!("../fixtures/recorded/market_2026-11-15.jsonl");

/// Prices to a hundredth of a cent and amounts to six decimals (`docs/integrations/polymarket.md`).
const PRICE: u8 = 4;
const QUANTITY: u8 = 6;

fn frames() -> Vec<&'static str> {
    FIXTURE.lines().filter(|l| !l.trim().is_empty()).collect()
}

fn scales() -> (Scale, Scale) {
    (Scale::new(PRICE).expect("price scale"), Scale::new(QUANTITY).expect("quantity scale"))
}

/// No frame may be refused. A parser that drops what it does not understand passes any fixture.
#[test]
fn every_recorded_frame_is_understood() {
    let (price, quantity) = scales();
    let (mut books, mut changes, mut pongs) = (0, 0, 0);
    for (n, frame) in frames().iter().enumerate() {
        let events = parse(frame.as_bytes(), price, quantity)
            .unwrap_or_else(|e| panic!("frame {n} was refused by the parser: {e}"));
        for event in events {
            match event {
                VenueEvent::Book(book) => {
                    assert!(!book.bids.is_empty() && !book.asks.is_empty(), "frame {n}");
                    books += 1;
                }
                VenueEvent::Changes { .. } => changes += 1,
                VenueEvent::Pong => pongs += 1,
                _ => {}
            }
        }
    }
    assert_eq!(books, 2, "the opening frame carries both outcomes' books");
    assert!(changes >= 20, "the recording carries a real run, found {changes} changes");
    assert!(pongs >= 1, "no pong was recorded, so the ping cycle is unproven");
}

/// A declaration serving depth and trades, so an emitted class the venue was not declared for
/// fails the replay.
fn declared() -> CapabilityDeclaration {
    let profile = serde_json::json!({
        "format": 1,
        "venue": "the venue under replay",
        "verified_at": null,
        "transports": { "stream": {
            "funding": { "model": "continuous" },
            "data_classes": ["depth", "trades"],
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

/// The recorded run, through the feed the live path uses, held to the record contract. Every
/// change agrees with the venue's stated best prices, or the feed would have refused it.
#[test]
fn every_record_from_the_recorded_run_keeps_the_record_contract() {
    let recorded = frames();
    let frames: Vec<RecordedFrame<'_>> = recorded
        .iter()
        .enumerate()
        .map(|(n, frame)| RecordedFrame {
            received_at_ns: 1_794_748_952_000_000_000 + n as i128,
            frame: frame.as_bytes(),
        })
        .collect();
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    let results = check_feed(&mut feed, &frames, &declared());

    for result in &results {
        assert!(result.is_pass(), "{}: {:?}", result.name, result.outcome);
    }
    assert_eq!(feed.instruments(), 2, "one book per outcome in the recording");
}
