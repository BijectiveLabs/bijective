// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::stream;
use bijective_event_store::store::StreamWriter;

/// Two venues never share a stream, and **the store accepts both names**.
/// Opens both rather than comparing strings, because the store refuses some names.
#[test]
fn each_venue_opens_its_own_stream() {
    let dir = std::env::temp_dir().join(format!("bl-stream-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");

    assert_ne!(stream("venue-a"), stream("venue-b"));

    let first = StreamWriter::open(&dir, &stream("venue-a")).expect("the store refused a name");
    let second = StreamWriter::open(&dir, &stream("venue-b")).expect("the store refused a name");

    drop(first);
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}

fn top(received_at_ns: Option<i128>) -> super::BookUpdate {
    super::BookUpdate {
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        seq: bijective_model::Sequence::Numbered(7),
        venue_ts_ms: 1_788_000_000_000,
        received_at_ns,
        bid_price: 2_654_375,
        bid_quantity: 1_500,
        ask_price: 2_654_500,
        ask_quantity: 900,
        bid_depth: 3,
        ask_depth: 4,
    }
}

/// A receive time, and its absence, survive the round trip. Absent is not zero.
#[test]
fn a_receive_time_round_trips_and_absent_stays_absent() {
    for update in [top(Some(1_788_000_000_000_250_000)), top(None)] {
        let bytes = super::encode(&update).expect("encode");
        assert_eq!(super::decode(&bytes).expect("decode"), update);
    }
}

/// A store recorded before receive times were kept is read as it is, with no receive time,
/// rather than refused: an upgrade never asks for a migration of the market record.
#[test]
fn a_version_3_top_is_read_without_a_receive_time() {
    let update = top(None);
    let mut w = crate::codec::Writer::new(crate::codec::ClassId::MarketBookUpdate, 3);
    w.str(update.instrument.as_str()).expect("instrument");
    w.u8(1).u64(7).u64(update.venue_ts_ms);
    w.i128(update.bid_price).i128(update.bid_quantity).i128(update.ask_price);
    w.i128(update.ask_quantity).u32(update.bid_depth).u32(update.ask_depth);
    assert_eq!(super::decode(&w.finish()).expect("a version 3 top"), update);
}
