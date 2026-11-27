// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Put book updates on the stream a strategy reads, for a drill that has no venue attached.
//! Prices climb by one so a strategy can notice what it was sent. An optional fourth argument widens each spread,
//! an optional fifth names the venue whose market stream is written (`drill` when absent), and an
//! optional sixth is the venue time of the first update in epoch milliseconds, for a drill whose
//! node judges how fresh the market is.

fn main() {
    let root = std::env::args().nth(1).expect("store root");
    let instrument = std::env::args().nth(2).expect("instrument identity");
    let count: u64 = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "3".to_string())
        .parse()
        .expect("how many updates");

    let widen: i128 = std::env::args()
        .nth(4)
        .unwrap_or_else(|| "0".to_string())
        .parse()
        .expect("how much wider each spread is than the last");

    let first_ms: u64 = std::env::args()
        .nth(6)
        .unwrap_or_else(|| "1788000000000".to_string())
        .parse()
        .expect("the first update's venue time in epoch milliseconds");

    let mut w = bijective_event_store::store::StreamWriter::open(
        std::path::Path::new(&root),
        &bijective_serialization::signal::market::stream(
            &std::env::args().nth(5).unwrap_or_else(|| "drill".to_string()),
        ),
    )
    .expect("open");

    for i in 0..count {
        let update = bijective_serialization::signal::market::BookUpdate {
            instrument: bijective_model::InstrumentId::new(instrument.as_str()),
            seq: bijective_model::Sequence::Numbered(i),
            venue_ts_ms: first_ms + i,
            // A fixed delay after the venue stamp, so a drill sees a receive time and stays deterministic.
            received_at_ns: Some((i128::from(first_ms) + i128::from(i)) * 1_000_000 + 1_000_000),
            bid_price: 2_654_375 + i128::from(i),
            bid_quantity: 1_500,
            ask_price: 2_654_500 + i128::from(i) + widen * i128::from(i),
            ask_quantity: 900,
            bid_depth: 5,
            ask_depth: 5,
        };
        w.append(&bijective_serialization::signal::market::encode(&update).expect("encode"))
            .expect("append");
    }

    println!(
        "seeded {count} book update(s) on {}",
        bijective_serialization::signal::market::stream("drill")
    );
}
