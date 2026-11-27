// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operational host program.

use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use bijective_common::clock::{Clock, WallClock};
use bijective_common::readiness::{Health, Surface};
use bijective_deployment::host::sweep;

const PROGRAM: &str = "operational-host";
const SWEEP: Duration = Duration::from_secs(5);

/// One clock per program: a direct time read would opt out of reproducibility.
fn clock() -> &'static WallClock {
    static CLOCK: std::sync::OnceLock<WallClock> = std::sync::OnceLock::new();
    CLOCK.get_or_init(WallClock::new)
}

fn now_ns() -> i128 {
    clock().now_ns()
}

fn main() -> std::process::ExitCode {
    let mut store_root: Option<PathBuf> = None;
    let mut readiness: Option<PathBuf> = None;
    let mut drill_seconds: Option<u64> = None;

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next();
        match (flag.as_str(), value) {
            ("--store", Some(v)) => store_root = Some(PathBuf::from(v)),
            ("--readiness", Some(v)) => readiness = Some(PathBuf::from(v)),
            ("--drill-seconds", Some(v)) => drill_seconds = v.parse().ok(),
            (other, _) => {
                eprintln!("{PROGRAM}: unusable argument {other}");
                return std::process::ExitCode::from(2);
            }
        }
    }

    let (Some(store_root), Some(readiness)) = (store_root, readiness) else {
        eprintln!("{PROGRAM}: --store and --readiness are required");
        return std::process::ExitCode::from(2);
    };

    let surface = Surface::new(&readiness, PROGRAM);
    // Checked: a node that cannot say whether it is healthy must not run.
    if let Err(e) = surface.report(Health::Starting, "about to sweep the store", now_ns()) {
        eprintln!("{PROGRAM}: the readiness surface could not be written: {e}");
        return std::process::ExitCode::from(2);
    }

    let mut elapsed = 0u64;
    loop {
        let found = sweep(&store_root);
        let health = if found.is_healthy() { Health::Ready } else { Health::Failed };
        let detail = if !found.is_healthy() {
            format!("{} segment(s) not clean of {}", found.damaged, found.total())
        } else if found.is_complete() {
            format!("{} segment(s) verified clean", found.clean)
        } else {
            // A torn tail is stated, not folded into clean: it tells a restarted node from one that never stopped.
            format!(
                "{} segment(s) verified clean, {} ending mid-entry",
                found.clean, found.with_torn_tail
            )
        };
        surface.report_or_warn(health, detail, now_ns());

        if drill_seconds.is_some_and(|d| elapsed >= d) {
            return std::process::ExitCode::SUCCESS;
        }
        sleep(SWEEP);
        elapsed += SWEEP.as_secs();
    }
}
