// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Records raw venue frames to a file for conformance fixtures.
//! Usage: record_frames <out.jsonl> <seconds> <coin>...

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_venue::contract::StreamRequest;
use bijective_venue::normalise::Scale;
use bijective_venue_hyperliquid::transport::{Connection, Signal};
use tokio::sync::mpsc;

/// Mainnet WebSocket URL from the venue's WebSocket documentation, read 2026-11-04.
const MAINNET_STREAM: &str = "wss://api.hyperliquid.xyz/ws";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out_path = args.next().ok_or("usage: record_frames <out.jsonl> <seconds> <coin>...")?;
    let seconds: u64 = args.next().ok_or("missing duration")?.parse()?;
    let coins: Vec<String> = args.collect();
    if coins.is_empty() {
        return Err("at least one coin".into());
    }

    let request = StreamRequest {
        endpoint: MAINNET_STREAM.to_string(),
        instruments: coins.clone(),
        // Wide enough for every listed coin: the venue gives each its own size decimals (5 for
        // BTC, 4 for ETH), and a frame narrower scales refuse is never marked seen. Recorded
        // frames are raw, so the scales shape only the session running alongside.
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            coins.iter().map(String::as_str),
            bijective_venue::profile::Scales { price: Scale::new(6)?, quantity: Scale::new(6)? },
        ),
        clock: Arc::new(WallClock::new()),
    };
    let pair = request.scales.uniform()?;
    let (tx, mut rx) = mpsc::channel::<Signal>(1024);
    let driver = tokio::spawn(Connection::new(&request, pair)?.run(tx));

    let mut file = std::fs::File::create(&out_path)?;
    let mut frames = 0u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            Some(signal) = rx.recv() => match signal {
                Signal::Frame { received_at_ns, frame, .. } => {
                    let line = serde_json::json!({ "at_ns": received_at_ns.to_string(), "frame": frame });
                    writeln!(file, "{line}")?;
                    frames += 1;
                }
                Signal::State(state) => eprintln!("state: {state:?}"),
            },
        }
    }

    file.flush()?;
    driver.abort();
    eprintln!("{frames} frame(s) written to {out_path}");
    Ok(())
}
