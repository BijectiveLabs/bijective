// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prints what a strategy proposed, for an operator reading a node after the fact.
//! Proposals cross into the Control Engine, whose own reader would report each as an unknown class.

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(stream)) = (args.next(), args.next()) else {
        eprintln!("usage: read_proposals <store-root> <stream>");
        std::process::exit(2);
    };

    match bijective_event_store::store::read_range(
        std::path::Path::new(&root),
        &stream,
        0,
        u64::MAX,
    ) {
        Ok(entries) => {
            println!("{} proposal record(s)", entries.len());
            for entry in entries {
                match bijective_serialization::signal::decode(&entry.payload) {
                    Ok(p) => println!(
                        "{:>4}  {} {} {:?} qty={} price={:?} cap={:?} at={} desk={} strategy={}",
                        entry.seq,
                        p.client_id,
                        p.instrument.as_str(),
                        p.side,
                        p.quantity_scaled,
                        p.price_scaled,
                        p.cap_scaled,
                        p.decided_at_ns,
                        p.desk,
                        p.strategy.as_deref().unwrap_or("-")
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
