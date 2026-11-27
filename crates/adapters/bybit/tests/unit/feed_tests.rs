// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The records the feed emits: both instants, best-first books and removals stated as removals.

use bijective_venue::contract::market::{
    BookLevel, BookSide, BookSnapshot, ExchangeTime, LevelChange,
};
use bijective_venue::contract::{FeedError, MarketFact, MarketFeed, RecordDefect, Sequence};

use super::Feed;

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

const RECEIVED: i128 = 1_787_469_688_200_000_000;

fn feed() -> Feed {
    Feed::new(at(2, 3))
}

fn frame(kind: &str, seq: u64, bids: &str, asks: &str) -> String {
    format!(
        r#"{{"topic":"orderbook.50.BTCUSDT","type":"{kind}","ts":1787469688129,"data":{{"s":"BTCUSDT","b":{bids},"a":{asks},"seq":{seq},"u":{seq}}}}}"#
    )
}

/// The wire is read in whatever order it came. The record states the book best first.
#[test]
fn a_snapshot_becomes_a_best_first_record_with_both_instants() {
    let mut f = feed();
    let snapshot = frame(
        "snapshot",
        10,
        r#"[["99.00","1.000"],["100.00","2.000"]]"#,
        r#"[["102.00","1.000"],["101.00","3.000"]]"#,
    );
    let offered = f.offer(snapshot.as_bytes(), RECEIVED).expect("understood");

    assert_eq!(offered.records.len(), 1);
    let record = &offered.records[0];
    assert_eq!(record.instrument.as_str(), "BTCUSDT");
    assert_eq!(record.exchange_time, ExchangeTime::At { at_ns: 1_787_469_688_129_000_000 });
    assert_eq!(record.received_at_ns, RECEIVED);
    let level = |price_scaled, quantity_scaled| BookLevel { price_scaled, quantity_scaled };
    assert_eq!(
        record.fact,
        MarketFact::BookSnapshot(BookSnapshot {
            sequence: Sequence::Numbered(10),
            bids: vec![level(10_000, 2_000), level(9_900, 1_000)],
            asks: vec![level(10_100, 3_000), level(10_200, 1_000)],
        })
    );
    assert_eq!(offered.top.expect("a top").bid_price, 10_000);
}

#[test]
fn a_delta_states_a_zero_quantity_as_a_removal() {
    let mut f = feed();
    let snapshot = frame("snapshot", 10, r#"[["100.00","2.000"]]"#, r#"[["101.00","3.000"]]"#);
    f.offer(snapshot.as_bytes(), RECEIVED).expect("understood");

    let delta = frame("delta", 11, r#"[["100.00","0"]]"#, r#"[["101.50","1.000"]]"#);
    let offered = f.offer(delta.as_bytes(), RECEIVED + 5).expect("understood");

    let MarketFact::BookDelta(d) = &offered.records[0].fact else {
        panic!("not a delta: {:?}", offered.records);
    };
    assert_eq!(d.sequence, Sequence::Numbered(11));
    assert_eq!(
        d.changes,
        vec![
            LevelChange::Remove { side: BookSide::Bid, price_scaled: 10_000 },
            LevelChange::Set { side: BookSide::Ask, price_scaled: 10_150, quantity_scaled: 1_000 },
        ]
    );
    assert_eq!(offered.records[0].received_at_ns, RECEIVED + 5);
}

/// The record contract is enforced at the adapter: a frame stamped at the epoch never leaves it.
#[test]
fn a_frame_whose_record_breaks_the_contract_is_refused_at_the_adapter() {
    let mut f = feed();
    let unstamped = frame("snapshot", 10, r#"[["100.00","2.000"]]"#, "[]")
        .replace(r#""ts":1787469688129"#, r#""ts":0"#);
    assert!(matches!(
        f.offer(unstamped.as_bytes(), RECEIVED),
        Err(FeedError::RecordRefused(RecordDefect::ExchangeTimeNotPositive { .. }))
    ));
}

/// A consumer of both sees the record before the top it produced.
#[tokio::test]
async fn the_stream_forwards_records_before_the_top_they_produced() {
    use bijective_venue::contract::MarketEvent;

    let mut f = feed();
    let snapshot = frame("snapshot", 10, r#"[["100.00","2.000"]]"#, r#"[["101.00","3.000"]]"#);
    let offered = f.offer(snapshot.as_bytes(), RECEIVED).expect("understood");

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    assert!(crate::stream::forward(&tx, offered).await);
    drop(tx);

    assert!(matches!(rx.recv().await, Some(MarketEvent::Record(_))));
    assert!(matches!(rx.recv().await, Some(MarketEvent::Observation(_))));
    assert!(rx.recv().await.is_none());
}
