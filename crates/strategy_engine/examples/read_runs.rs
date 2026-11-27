// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prints the run ledger, for an operator asking what this node has run.
//! Read-only and lock-free, so it is safe against a running node.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: read_runs <store-root>");
        std::process::exit(2);
    };

    let stream = bijective_strategy_engine::ledger::RUNS;
    match bijective_event_store::store::read_range(std::path::Path::new(&root), stream, 0, u64::MAX)
    {
        Ok(entries) => {
            // Counted by kind: the stream holds two, and a mixed total would count an admission as a run.
            let admissions = entries
                .iter()
                .filter(|e| {
                    bijective_strategy_engine::runs::ledger::decode_admitted(&e.payload).is_ok()
                })
                .count();
            println!("{} run(s) recorded, {admissions} admission(s)", entries.len() - admissions);
            for entry in entries {
                if let Ok(landed) =
                    bijective_strategy_engine::runs::ledger::decode_admitted(&entry.payload)
                {
                    println!(
                        "{:>4}  {} admitted {}..={} ({}) on {} through {}",
                        entry.seq,
                        &landed.specification.to_hex()[..16],
                        landed.from,
                        landed.through,
                        landed.count,
                        landed.market,
                        landed.market_through
                    );
                    continue;
                }
                match bijective_strategy_engine::runs::ledger::decode(&entry.payload) {
                    Ok(run) => println!(
                        "{:>4}  {} {} {:?} started={} ended={} emitted={} strategy={}",
                        entry.seq,
                        &run.specification.to_hex()[..16],
                        run.class,
                        run.exit,
                        run.started_ns,
                        run.ended_ns,
                        &run.emitted.to_hex()[..16],
                        run.strategy
                            .map_or("unrecorded".to_string(), |s| s.to_hex()[..16].to_string())
                    ),
                    Err(e) => println!("{:>4}  unreadable: {e:?}", entry.seq),
                }
            }
        }
        Err(e) => {
            eprintln!("{e:?}");
            std::process::exit(1);
        }
    }
}
