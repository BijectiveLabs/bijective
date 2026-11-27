// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The client order identifier this node sends a venue, assigned before the network call.
//!
//! A strategy names a proposal with its own identifier, which is told back to it. A venue refuses
//! an identifier it has held before, and a strategy's is unique only within its own sessions, so
//! the venue is sent one this node makes instead: a function of the admission instant on this
//! node's clock and the proposal's position, at most 30 characters of lowercase letters, digits
//! and `-`. Written in the intent before the submit, so a retry or a restart reads it back.

/// `bl-<instant>-<position>`, both in base 36.
#[must_use]
pub fn client_order_id(admitted_at_ns: i128, from_position: u64) -> String {
    // Before the epoch or past 2554 reads as zero. The position still tells orders apart.
    let instant = u64::try_from(admitted_at_ns).unwrap_or(0);
    format!("bl-{}-{}", base36(instant), base36(from_position))
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}
