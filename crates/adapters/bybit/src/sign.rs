// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Request signing: HMAC_SHA256(secret, timestamp_ms || api_key || recv_window || payload), hex.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The venue's own bound: wider extends replay validity, narrower mistakes clock drift for an attack.
pub const RECV_WINDOW_MS: u64 = 5_000;

/// Separate from signing so the ordering can be checked against a known vector without a secret.
pub fn signing_payload(timestamp_ms: u64, api_key: &str, body: &str) -> String {
    format!("{timestamp_ms}{api_key}{RECV_WINDOW_MS}{body}")
}

/// Takes the secret as bytes from the broker rather than reading one.
pub fn sign(secret: &[u8], timestamp_ms: u64, api_key: &str, body: &str) -> String {
    hmac_hex(secret, &signing_payload(timestamp_ms, api_key, body))
}

/// The private stream's authentication payload: `GET/realtime` followed by the expiry in
/// milliseconds (V5 WebSocket connect guide, Authentication).
pub fn realtime_payload(expires_ms: u64) -> String {
    format!("GET/realtime{expires_ms}")
}

pub fn sign_realtime(secret: &[u8], expires_ms: u64) -> String {
    hmac_hex(secret, &realtime_payload(expires_ms))
}

fn hmac_hex(secret: &[u8], message: &str) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret)
        .expect("hmac accepts a key of any length");
    mac.update(message.as_bytes());
    hex_lower(&mac.finalize().into_bytes())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}
