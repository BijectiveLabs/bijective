// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! `read_probe <url> <subscribe frame> <seconds>`: connects, sends the frame, and prints every
//! ten seconds how many messages and bytes arrived, how far behind the venue the newest message
//! carrying a `"ts"` field (milliseconds) was read, and whether the venue compresses, so a link can
//! be measured with the client the adapters use.

use std::time::{Duration, Instant};

use bijective_venue_socket::{Inbound, Limits, connect};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (url, frame) = (&args[1], &args[2]);
    let seconds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
    let limits = Limits { connect_timeout: Duration::from_secs(10), max_message_bytes: 1 << 22 };
    let mut socket = connect(url, limits).await.expect("connected");
    println!("compressed: {}", socket.is_compressed());
    socket.send_text(frame.clone()).await.expect("sent");
    let start = Instant::now();
    let (mut messages, mut bytes, mut mark, mut behind_ms) = (0u64, 0u64, Instant::now(), 0i128);
    while start.elapsed() < Duration::from_secs(seconds) {
        match socket.next().await {
            Inbound::Text(text) => {
                messages += 1;
                bytes += text.len() as u64;
                if let Some(at) = venue_ms(&text) {
                    behind_ms = now_ms() - at;
                }
            }
            Inbound::Binary(_) | Inbound::Pong => {}
            Inbound::Closed => {
                println!("closed after {messages} messages");
                return;
            }
        }
        if mark.elapsed() >= Duration::from_secs(10) {
            let at = start.elapsed().as_secs_f64();
            println!(
                "{at:>5.1}s messages {messages} text kB {} behind ms {behind_ms}",
                bytes / 1024
            );
            mark = Instant::now();
        }
    }
    socket.close().await;
}

fn now_ms() -> i128 {
    use bijective_common::clock::{Clock, WallClock};
    WallClock::new().now_ns() / 1_000_000
}

/// The first `"ts":<digits>` in `text`.
fn venue_ms(text: &str) -> Option<i128> {
    let rest = &text[text.find("\"ts\":")? + 5..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
