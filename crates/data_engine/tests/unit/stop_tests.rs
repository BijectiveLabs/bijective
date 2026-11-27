// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

#![cfg(unix)]

use std::time::Duration;

use super::{install, requested};

/// The drill's race: the signal lands before any loop waits on it, and must still end the program.
#[tokio::test]
async fn a_stop_sent_before_anything_waits_is_still_seen() {
    install().unwrap();
    let sent = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(std::process::id().to_string())
        .status()
        .unwrap();
    assert!(sent.success());
    tokio::time::sleep(Duration::from_millis(200)).await;

    let seen = tokio::time::timeout(Duration::from_secs(5), requested()).await;
    assert!(seen.is_ok(), "the stop sent before the wait was lost");

    // Latched: every later wait sees the same request at once.
    let again = tokio::time::timeout(Duration::from_millis(100), requested()).await;
    assert!(again.is_ok(), "a second wait did not see the request");
}
