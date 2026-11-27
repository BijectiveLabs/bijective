// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prints what a venue's facts stream holds, one fact per line, for a drill to assert on.
//! Arguments: store root, venue.

use bijective_model::market::MarketFact;
use bijective_serialization::signal::facts::{decode, stream};

fn main() {
    let root = std::env::args().nth(1).expect("store root");
    let venue = std::env::args().nth(2).expect("venue");
    let entries = bijective_event_store::store::read_range(
        std::path::Path::new(&root),
        &stream(&venue),
        0,
        u64::MAX,
    )
    .expect("the facts stream");
    for entry in entries {
        let read = decode(&entry.payload).expect("a fact");
        let kind = match read.fact {
            MarketFact::Trade(_) => "trade",
            MarketFact::Liquidation(_) => "liquidation",
            MarketFact::Funding(_) => "funding",
            MarketFact::OpenInterest(_) => "open_interest",
            MarketFact::MarkPrice { .. } => "mark_price",
            MarketFact::IndexPrice { .. } => "index_price",
            MarketFact::LongShortRatio(_) => "long_short_ratio",
            MarketFact::Resolution { .. } => "resolution",
            MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_) => "book",
        };
        println!(
            "{} {kind} {} received {}",
            entry.seq,
            read.instrument.as_str(),
            read.received_at_ns
        );
    }
}
