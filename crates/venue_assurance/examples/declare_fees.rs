// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Declare what a venue charges. Refused unless the standing check confirms the venue.
//! Figures are in basis points in the platform's fixed scale: 5.5 basis points is 55000.

use bijective_serialization::signal::fees::Fees;
use bijective_venue_assurance::wire::declare_fees;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [root, venue, taker, maker, source, read_at_ns] = a.as_slice() else {
        eprintln!(
            "usage: declare_fees <store> <venue> <taker> <maker> <source> <read-at-ns>\n\
             \x20      figures in basis points in the fixed scale, positive is a cost"
        );
        std::process::exit(2);
    };

    let fees = Fees {
        venue: venue.clone(),
        taker: taker.parse().expect("a fee in the fixed scale"),
        maker: maker.parse().expect("a fee in the fixed scale"),
        source: source.clone(),
        read_at_ns: read_at_ns.parse().expect("an instant in nanoseconds"),
    };

    match declare_fees(std::path::Path::new(root), &fees) {
        Ok(at) => println!("declared for {venue} at position {at}"),
        Err(e) => {
            eprintln!("refused: {e}");
            std::process::exit(1);
        }
    }
}
