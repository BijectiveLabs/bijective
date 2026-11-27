// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Records raw venue frames to a file for conformance fixtures.
//! Usage: record_frames <out.jsonl> <seconds> <symbol>...

use std::io::Write;
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_venue::contract::StreamRequest;
use bijective_venue::normalise::Scale;
use bijective_venue_bybit::{Captured, Connection, Signal};
use tokio::sync::mpsc;

const PUBLIC_LINEAR_STREAM: &str = "wss://stream.bybit.com/v5/public/linear";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out_path = args.next().ok_or("usage: record_frames <out.jsonl> <seconds> <symbol>...")?;
    let seconds: u64 = args.next().ok_or("missing duration")?.parse()?;
    let symbols: Vec<String> = args.collect();
    if symbols.is_empty() {
        return Err("at least one symbol".into());
    }

    let topics: Vec<String> = symbols.iter().map(|s| format!("orderbook.50.{s}")).collect();

    // Scales matter only to the parser running alongside. Recorded frames are raw.
    let price = Scale::new(2)?;
    let quantity = Scale::new(3)?;

    let (sig_tx, mut sig_rx) = mpsc::channel::<Signal>(1024);
    let (cap_tx, mut cap_rx) = mpsc::channel::<Captured>(8192);
    let (_notice_tx, notice_rx) = mpsc::channel(8);

    let request = StreamRequest {
        endpoint: PUBLIC_LINEAR_STREAM.to_string(),
        instruments: symbols.clone(),
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            symbols.iter().map(String::as_str),
            bijective_venue::profile::Scales { price, quantity },
        ),
        clock: std::sync::Arc::new(WallClock::new()),
    };
    let conn = Connection::new(&request, topics)?.with_capture(cap_tx);

    let driver = tokio::spawn(conn.run(sig_tx, notice_rx));

    let mut file = std::fs::File::create(&out_path)?;
    let mut frames = 0u64;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            Some(c) = cap_rx.recv() => {
                let line = serde_json::json!({ "at_ns": c.at_ns.to_string(), "frame": c.frame });
                writeln!(file, "{line}")?;
                frames += 1;
            }
            Some(s) = sig_rx.recv() => {
                if let Signal::State(state) = s {
                    eprintln!("state: {state:?}");
                }
            }
        }
    }

    file.flush()?;
    driver.abort();
    eprintln!("{frames} frame(s) written to {out_path}");
    Ok(())
}
