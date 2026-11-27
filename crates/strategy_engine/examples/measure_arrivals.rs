// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reads a venue's arrival rate from the venue timestamps of a recorded stream.

use std::collections::BTreeMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(stream)) = (args.next(), args.next()) else {
        eprintln!("usage: measure-arrivals <store-root> <market-stream> [queue-bound]");
        std::process::exit(2);
    };
    let bound: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1024);

    let entries = match bijective_event_store::store::read_range(
        std::path::Path::new(&root),
        &stream,
        0,
        u64::MAX,
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("measure-arrivals: {stream} could not be read: {e:?}");
            std::process::exit(1);
        }
    };

    // Venue clock, not ours: a strategy keeps up with the rate the venue emits, not our receive jitter.
    let mut per_second: BTreeMap<i64, u64> = BTreeMap::new();
    let mut readable = 0u64;
    for entry in &entries {
        let Ok(update) = bijective_serialization::signal::market::decode(&entry.payload) else {
            continue;
        };
        readable += 1;
        *per_second.entry((update.venue_ts_ms / 1000) as i64).or_insert(0) += 1;
    }

    if per_second.is_empty() {
        eprintln!("measure-arrivals: nothing readable on {stream}");
        std::process::exit(1);
    }

    let seconds = per_second.len() as u64;
    let busiest = per_second.values().copied().max().unwrap_or(0);
    let quietest = per_second.values().copied().min().unwrap_or(0);
    let mean = readable / seconds.max(1);

    println!("stream    {stream}");
    println!("read      {readable} of {} entr(ies), over {seconds} second(s)", entries.len());
    println!("mean      {mean} event(s)/s");
    println!("busiest   {busiest} event(s) in one second");
    println!("quietest  {quietest} event(s) in one second");
    println!();

    println!("against a queue of {bound}:");
    println!(
        "  at the mean      {} ms of stall before a session ends",
        bound.saturating_mul(1000) / mean.max(1)
    );
    println!("  at the busiest   {} ms", bound.saturating_mul(1000) / busiest.max(1));
    if busiest >= bound {
        println!("  a single second of this venue's busiest traffic fills the queue on its own");
    }
}
