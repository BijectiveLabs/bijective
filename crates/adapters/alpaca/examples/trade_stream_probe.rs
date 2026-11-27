// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Starts the trade updates link against the venue and prints what it says each half second for
//! a bounded time. The key id and the secret key are read from standard input, one per line.
//! Nothing secret is printed.
//! Usage: trade_stream_probe <stream-url> <seconds>

use std::io::BufRead;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bijective_common::clock::WallClock;
use bijective_venue_alpaca::trade_stream::{LinkSetup, TradeLink};

fn line(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> String {
    let text = lines.next().and_then(Result::ok).unwrap_or_default();
    text.trim().trim_matches('"').trim_matches('\'').to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, url, seconds] = args.as_slice() else {
        eprintln!("usage: trade_stream_probe <stream-url> <seconds>");
        std::process::exit(2);
    };
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let key_id = line(&mut lines);
    let secret = line(&mut lines);
    let link = TradeLink::start(LinkSetup {
        url: url.clone(),
        key_id,
        secret,
        clock: Arc::new(WallClock::new()),
    })
    .expect("a thread");
    let start = Instant::now();
    let bound = Duration::from_secs(seconds.parse().expect("seconds"));
    let mut last = None;
    while start.elapsed() < bound {
        let pushed = link.pushed();
        if last != Some(pushed) {
            println!("{:>5} ms: {pushed:?}", start.elapsed().as_millis());
            last = Some(pushed);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
