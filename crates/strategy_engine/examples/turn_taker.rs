// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A strategy that answers every event, for measuring what an exchange costs.
//! Compiled, not a shell loop: a shell reads one byte at a time and would measure itself.

use std::io::{BufRead, Write};

use bijective_strategy_engine::protocol::{Inbound, read_event};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut seen: u64 = 0;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        match read_event(&line) {
            Ok(Inbound::Book { .. }) => {
                seen += 1;
                // The stepwise exchange waits on this line. The whole exchange passes over it.
                if writeln!(out, "{{\"emit\":\"answered\"}}").is_err() || out.flush().is_err() {
                    return;
                }
            }
            Ok(Inbound::EndOfWindow) => break,
            _ => {}
        }
    }

    let _ = writeln!(
        out,
        "{{\"emit\":\"proposal\",\"desk\":\"alpha\",\"instrument\":\"btc-perp\",\"side\":\"buy\",\"quantity_scaled\":\"1\",\"price_scaled\":\"{seen}\",\"client_id\":\"turn-taker\",\"decided_at_ns\":\"1\"}}"
    );
    let _ = out.flush();
}
