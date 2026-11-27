// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Records raw venue frames to a file for conformance fixtures.
//! Usage: record_frames <out.jsonl> <seconds> <asset id>...
//! Each frame is also run through the feed, and the run ends with what the feed made of them.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_venue::contract::StreamRequest;
use bijective_venue::normalise::Scale;
use bijective_venue_polymarket::Feed;
use bijective_venue_polymarket::transport::{Connection, Signal};
use tokio::sync::mpsc;

/// The market stream from the venue's real-time data documentation, read 2026-11-15.
const MARKET_STREAM: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out_path = args.next().ok_or("usage: record_frames <out.jsonl> <seconds> <asset id>...")?;
    let seconds: u64 = args.next().ok_or("missing duration")?.parse()?;
    let coins: Vec<String> = args.collect();
    if coins.is_empty() {
        return Err("at least one asset id".into());
    }

    let request = StreamRequest {
        endpoint: MARKET_STREAM.to_string(),
        instruments: coins.clone(),
        // A hundredth of a cent and six-decimal amounts (`docs/integrations/polymarket.md`).
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            coins.iter().map(String::as_str),
            bijective_venue::profile::Scales { price: Scale::new(4)?, quantity: Scale::new(6)? },
        ),
        clock: Arc::new(WallClock::new()),
    };
    let pair = request.scales.uniform()?;
    let (tx, mut rx) = mpsc::channel::<Signal>(1024);
    let (lost_tx, lost_rx) = mpsc::channel::<String>(256);
    let driver = tokio::spawn(Connection::new(&request, pair)?.run(tx, lost_rx));

    let mut file = std::fs::File::create(&out_path)?;
    let mut frames = 0u64;
    let mut feed = Feed::new(pair.price, pair.quantity);
    let (mut tops, mut refused) = (0u64, 0u64);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            Some(signal) = rx.recv() => match signal {
                Signal::Frame { received_at_ns, frame, parsed } => {
                    match parsed.map_err(|e| e.to_string()).and_then(|events| {
                        feed.observe(&events, received_at_ns).map_err(|e| e.to_string())
                    }) {
                        Ok(offered) => tops += u64::from(offered.top.is_some()),
                        Err(e) => {
                            refused += 1;
                            eprintln!("refused: {e}");
                            if let Some(asset) = e.split(" on ").nth(1) {
                                let _ = lost_tx.try_send(asset.to_string());
                            }
                        }
                    }
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
    eprintln!("{frames} frame(s) written to {out_path}; {tops} top(s), {refused} refused");
    Ok(())
}
