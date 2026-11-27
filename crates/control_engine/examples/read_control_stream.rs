// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Print what the Control Engine recorded. Read-only: takes no lock, safe against a running node.

use bijective_control_engine::wire::decode;
use bijective_event_store::store::read_range;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(stream)) = (args.next(), args.next()) else {
        eprintln!("usage: read_control_stream <store-root> <stream>");
        std::process::exit(2);
    };

    match read_range(std::path::Path::new(&root), &stream, 0, u64::MAX) {
        Ok(entries) => {
            println!("{} entr(ies)", entries.len());
            for entry in entries {
                match decode(&entry.payload) {
                    Ok(event) => println!("{:>4}  {event:?}", entry.seq),
                    Err(e) => println!("{:>4}  unreadable: {e}", entry.seq),
                }
            }
        }
        Err(e) => {
            eprintln!("{e:?}");
            std::process::exit(1);
        }
    }
}
