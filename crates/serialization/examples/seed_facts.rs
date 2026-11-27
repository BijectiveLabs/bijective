// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Put mark-price facts on a venue's facts stream, for a drill that has no venue attached.
//! Arguments: store root, instrument, count, venue. Each fact arrives at the instant
//! `seed_market` gives the top of the same index, as a frame's fact and top would.

use bijective_model::market::{ExchangeTime, MarketFact};
use bijective_serialization::signal::facts::{FactUpdate, encode, stream};

fn main() {
    let root = std::env::args().nth(1).expect("store root");
    let instrument = std::env::args().nth(2).expect("instrument identity");
    let count: u64 = std::env::args().nth(3).expect("how many facts").parse().expect("a count");
    let venue = std::env::args().nth(4).expect("venue");

    let mut w = bijective_event_store::store::StreamWriter::open(
        std::path::Path::new(&root),
        &stream(&venue),
    )
    .expect("open");
    for i in 0..count {
        let stamped = 1_788_000_000_000 + i128::from(i);
        let update = FactUpdate {
            instrument: bijective_model::InstrumentId::new(instrument.as_str()),
            exchange_time: ExchangeTime::At { at_ns: stamped * 1_000_000 },
            received_at_ns: stamped * 1_000_000 + 1_000_000,
            fact: MarketFact::MarkPrice { mark_price_scaled: 2_654_400 + i128::from(i) },
        };
        w.append(&encode(&update).expect("encode")).expect("append");
    }
    println!("seeded {count} fact(s) on {}", stream(&venue));
}
