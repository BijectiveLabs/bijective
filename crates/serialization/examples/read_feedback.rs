// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prints what the order feedback mirroring control stream `<control>` holds, one record per line,
//! for a drill to assert on.

use bijective_serialization::signal::feedback::{decode, stream_for};

fn main() {
    let root = std::env::args().nth(1).expect("store root");
    let control = std::env::args().nth(2).unwrap_or_else(|| "control".to_string());
    let entries = bijective_event_store::store::read_range(
        std::path::Path::new(&root),
        &stream_for(&control),
        0,
        u64::MAX,
    )
    .expect("the feedback stream");
    for entry in entries {
        let told = decode(&entry.payload).expect("a feedback record");
        println!(
            "proposal {} {} control {}",
            told.proposal_position,
            told.outcome.name(),
            told.control_position
        );
    }
}
