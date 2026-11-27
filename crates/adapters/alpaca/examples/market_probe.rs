// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Runs the market stream against the venue for a bounded time and prints what arrived: link
//! states, records by kind, tops and refusals. The key id and the secret key are read from
//! standard input, one per line. Nothing secret is printed.
//! Usage: market_probe <stream-url> <symbol,...> <seconds> [record-path]
//! With a record path, every frame is written to it as it arrived, one per line.

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{MarketEvent, MarketFact, StreamRequest};
use bijective_venue::normalise::Scale;

fn line(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> String {
    let text = lines.next().and_then(Result::ok).unwrap_or_default();
    text.trim().trim_matches('"').trim_matches('\'').to_string()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (url, symbols, seconds) = match args.as_slice() {
        [_, url, symbols, seconds, ..] => (url, symbols, seconds),
        _ => {
            eprintln!("usage: market_probe <stream-url> <symbol,...> <seconds> [record-path]");
            std::process::exit(2);
        }
    };
    let mut recording = args.get(4).map(|path| std::fs::File::create(path).expect("a record file"));
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let key_id = line(&mut lines);
    let secret = Credential::new(line(&mut lines).into_bytes());
    let request = StreamRequest {
        endpoint: url.clone(),
        instruments: symbols.split(',').map(str::to_string).collect(),
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            symbols.split(','),
            bijective_venue::profile::Scales {
                price: Scale::new(4).expect("price scale"),
                quantity: Scale::new(0).expect("quantity scale"),
            },
        ),
        clock: Arc::new(WallClock::new()),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MarketEvent>(4096);
    tokio::spawn(async move {
        let ended = bijective_venue_alpaca::run_market_stream(&request, &key_id, &secret, tx).await;
        eprintln!("stream ended: {ended:?}");
    });
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(seconds.parse().expect("seconds"));
    let (mut quotes, mut trades, mut tops, mut frames) = (0, 0, 0, 0);
    loop {
        let event = tokio::select! {
            event = rx.recv() => event,
            () = tokio::time::sleep_until(deadline) => None,
        };
        match event {
            Some(MarketEvent::Received(received)) => {
                frames += 1;
                if let Some(file) = recording.as_mut() {
                    use std::io::Write;
                    file.write_all(&received.frame).expect("written");
                    file.write_all(b"\n").expect("written");
                }
            }
            Some(MarketEvent::Record(record)) => match record.fact {
                MarketFact::BookSnapshot(_) => quotes += 1,
                MarketFact::Trade(_) => trades += 1,
                _ => {}
            },
            Some(MarketEvent::Observation(top)) => {
                tops += 1;
                if tops == 1 {
                    println!("first top: {top:?}");
                }
            }
            Some(MarketEvent::State(state)) => println!("state: {state:?}"),
            Some(MarketEvent::Refused(e)) => println!("refused: {e}"),
            Some(MarketEvent::ContinuityLost) => println!("continuity lost"),
            None => break,
        }
    }
    println!("{frames} frame(s), {quotes} quote(s), {trades} trade(s), {tops} top(s)");
}
