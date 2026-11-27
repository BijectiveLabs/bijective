// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prints the venues this build carries, one per line, so drills do not hardcode a name.

fn main() {
    for venue in bijective_venue_registry::REGISTERED {
        println!("{venue}");
    }
}
