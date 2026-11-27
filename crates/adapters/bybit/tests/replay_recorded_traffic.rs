// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Replays recorded live traffic through the parser and the book.
//! A mock encodes an assumption about the venue. A recording is what the venue sent.

use std::collections::HashMap;

use bijective_venue::contract::MarketFeed;
use bijective_venue::contract::conformance::{RecordedFrame, check_feed};
use bijective_venue::normalise::Scale;
use bijective_venue::profile::{CapabilityDeclaration, VenueProfile};
use bijective_venue_bybit::{BookUpdateKind, Feed, L2Book, VenueMessage, parse};
use serde::Deserialize;

const FIXTURE: &str = include_str!("../fixtures/bybit_linear_orderbook50.jsonl");

#[derive(Deserialize)]
struct Recorded {
    at_ns: String,
    frame: String,
}

struct Tally {
    snapshots: usize,
    deltas: usize,
    acks: usize,
    pongs: usize,
    unrecognised: usize,
}

fn replay() -> (Tally, HashMap<String, L2Book>) {
    let price = Scale::new(2).expect("price scale");
    let quantity = Scale::new(3).expect("quantity scale");

    let mut books: HashMap<String, L2Book> = HashMap::new();
    let mut t = Tally { snapshots: 0, deltas: 0, acks: 0, pongs: 0, unrecognised: 0 };

    for (n, line) in FIXTURE.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: Recorded = serde_json::from_str(line).expect("fixture line is readable");
        let msg = parse(
            &rec.frame,
            &bijective_venue::profile::InstrumentScales::uniform_over(
                ["AAAUSDT", "BTCUSDT", "ETHUSDT"],
                bijective_venue::profile::Scales { price, quantity },
            ),
        )
        .unwrap_or_else(|e| panic!("frame {n} refused: {e}"));

        match msg {
            VenueMessage::Book { symbol, kind, bids, asks, seq, .. } => {
                let book = books.entry(symbol.clone()).or_default();
                match kind {
                    BookUpdateKind::Snapshot => {
                        book.apply_snapshot(&bids, &asks, seq);
                        t.snapshots += 1;
                    }
                    BookUpdateKind::Delta => {
                        book.apply_delta(&bids, &asks, seq)
                            .unwrap_or_else(|e| panic!("frame {n} on {symbol}: {e:?}"));
                        t.deltas += 1;
                    }
                }
                assert!(!book.is_crossed(), "frame {n} left {symbol} crossed");
            }
            VenueMessage::SubscribeResult { success, .. } => {
                assert!(success, "frame {n} is a refused subscription");
                t.acks += 1;
            }
            VenueMessage::Pong => t.pongs += 1,
            VenueMessage::Unrecognised { .. } => t.unrecognised += 1,
            VenueMessage::Trades { .. }
            | VenueMessage::Liquidations { .. }
            | VenueMessage::Ticker { .. } => {
                panic!("frame {n}: the recorded book traffic holds no trades or tickers")
            }
        }
    }

    (t, books)
}

/// No frame may be refused or quietly skipped. A parser that drops what it does not understand passes any fixture.
#[test]
fn every_recorded_frame_is_understood() {
    let (t, _) = replay();
    assert_eq!(t.unrecognised, 0, "a recorded frame was not recognised");
    assert!(t.snapshots >= 1 && t.deltas > 100, "the fixture carries a real run");
}

/// The per-connection sequence jumps between one symbol's deltas. Only real traffic proves the rule holds.
#[test]
fn the_books_survive_the_whole_recorded_run() {
    let (t, books) = replay();

    assert!(!books.is_empty());
    for (symbol, book) in &books {
        assert!(book.is_initialised(), "{symbol} never took a snapshot");
        let (bids, asks) = book.depth();
        assert!(bids > 0 && asks > 0, "{symbol} ended empty");
        assert!(book.best_bid().is_some() && book.best_ask().is_some());
    }
    assert!(t.deltas > t.snapshots);
}

/// The confirmed-subscription rule's confirmation and the ping cycle both appear in a recording long enough to contain them.
#[test]
fn the_recording_covers_the_choreography_and_not_only_the_data() {
    let (t, _) = replay();
    assert!(t.acks >= 1, "no subscribe acknowledgement was recorded");
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
    let recorded: Vec<(i128, String)> = FIXTURE
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let rec: Recorded = serde_json::from_str(l).expect("fixture line is readable");
            (rec.at_ns.parse().expect("arrival time"), rec.frame)
        })
        .collect();
    let frames: Vec<RecordedFrame<'_>> = recorded
        .iter()
        .map(|(at, frame)| RecordedFrame { received_at_ns: *at, frame: frame.as_bytes() })
        .collect();

    let price = Scale::new(2).expect("price scale");
    let quantity = Scale::new(3).expect("quantity scale");
    let mut feed = Feed::new(bijective_venue::profile::InstrumentScales::uniform_over(
        ["AAAUSDT", "BTCUSDT", "ETHUSDT"],
        bijective_venue::profile::Scales { price, quantity },
    ));
    let results = check_feed(&mut feed, &frames, &depth_only());

    assert_eq!(results.len(), 5);
    for result in &results {
        assert!(result.is_pass(), "{result:?}");
    }
    let (t, _) = replay();
    assert_eq!(feed.accepted() as usize, t.snapshots + t.deltas);
}
