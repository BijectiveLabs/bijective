// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prints each top of book on a market stream with its venue stamp and its receipt, one per line,
//! and the largest gap between venue stamps per instrument, for a drill or a measurement.
//! Arguments: store root, venue.

use std::collections::BTreeMap;

use bijective_serialization::signal::market::{decode, stream};

fn main() {
    let root = std::env::args().nth(1).expect("store root");
    let venue = std::env::args().nth(2).expect("venue");
    let entries = bijective_event_store::store::read_range(
        std::path::Path::new(&root),
        &stream(&venue),
        0,
        u64::MAX,
    )
    .expect("the market stream");
    let mut last: BTreeMap<String, u64> = BTreeMap::new();
    let mut widest: BTreeMap<String, u64> = BTreeMap::new();
    for entry in entries {
        let top = decode(&entry.payload).expect("a top of book");
        let id = top.instrument.as_str().to_string();
        if let Some(before) = last.insert(id.clone(), top.venue_ts_ms) {
            let gap = top.venue_ts_ms.saturating_sub(before);
            let held = widest.entry(id.clone()).or_insert(0);
            *held = (*held).max(gap);
        }
        let received = top.received_at_ns.map_or(-1, |r| r / 1_000_000);
        println!("{} {id} venue_ms {} received_ms {received}", entry.seq, top.venue_ts_ms);
    }
    for (id, gap) in widest {
        println!("widest {id} {gap} ms");
    }
}
