// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order path after a research run (`docs/decisions/0015-backtest-through-the-order-path.md`):
//! the run's proposals are copied into a record of their own, and the control engine judges,
//! places and fills them on simulated venues over the same recording.

use std::path::{Path, PathBuf};

use bijective_event_store::store::{StreamWriter, end_of, read_range};
use bijective_serialization::signal::node::BACKTEST;
use bijective_strategy_engine::runs::RESEARCH_PROPOSALS;

/// The last position on the research proposal stream, before a run appends to it.
pub(crate) fn proposals_end(store: &Path) -> Result<Option<u64>, String> {
    end_of(store, RESEARCH_PROPOSALS).map_err(|e| format!("{RESEARCH_PROPOSALS}: {e:?}"))
}

/// What one research run left for the order path to judge.
pub(crate) struct Ran<'a> {
    pub(crate) store: &'a Path,
    pub(crate) market: &'a str,
    pub(crate) config: &'a Path,
    /// The proposal stream's end before the run: the run wrote what follows it.
    pub(crate) before: Option<u64>,
}

/// Runs the control engine over the proposals `ran` wrote. `Ok(false)` when the control engine
/// stopped on a failure it has already stated.
pub(crate) fn run(ran: &Ran<'_>, control_engine: &Path) -> Result<bool, String> {
    let Some(through) = proposals_end(ran.store)? else {
        println!("order path: the run proposed nothing, so there is nothing to place");
        return Ok(true);
    };
    let from = ran.before.map_or(0, |b| b + 1);
    if from > through {
        println!("order path: the run proposed nothing, so there is nothing to place");
        return Ok(true);
    }
    let record = ran.store.join("run").join("backtests").join(format!("{from}-{through}"));
    if record.exists() {
        return Err(format!("{} already holds a backtest; it is never added to", record.display()));
    }
    copy_proposals(ran.store, &record, (from, through))?;
    let start_ns = recording_starts_at(ran.store, ran.market)?;
    println!("order path: proposals {from} to {through}, judged on simulated venues");
    println!("record:   {}", record.display());
    println!();

    let status = std::process::Command::new(control_engine)
        .arg("--mode")
        .arg("backtest")
        .arg("--replay-from")
        .arg(start_ns.to_string())
        .arg("--config")
        .arg(ran.config)
        .arg("--store")
        .arg(&record)
        .arg("--market-store")
        .arg(ran.store)
        .arg("--market")
        .arg(ran.market)
        .arg("--readiness")
        .arg(record.join("readiness.json"))
        .status()
        .map_err(|e| format!("the control engine could not be started: {e}"))?;
    Ok(status.success())
}

/// The run's proposals, byte for byte, on the stream the backtest node reads.
fn copy_proposals(store: &Path, record: &Path, (from, through): (u64, u64)) -> Result<(), String> {
    std::fs::create_dir_all(record)
        .map_err(|e| format!("{} could not be made: {e}", record.display()))?;
    let entries = read_range(store, RESEARCH_PROPOSALS, from, through)
        .map_err(|e| format!("{RESEARCH_PROPOSALS} could not be read: {e:?}"))?;
    let mut writer = StreamWriter::open(record, BACKTEST.proposals)
        .map_err(|e| format!("{} could not be opened: {e:?}", BACKTEST.proposals))?;
    for entry in entries {
        writer
            .append(&entry.payload)
            .map_err(|e| format!("a proposal could not be copied: {e:?}"))?;
    }
    Ok(())
}

/// The instant the recording's first update was received, where the backtest's clock starts.
fn recording_starts_at(store: &Path, market: &str) -> Result<i128, String> {
    let first = read_range(store, market, 0, 0).map_err(|e| format!("{market}: {e:?}"))?;
    let Some(entry) = first.first() else {
        return Err(format!("{market} holds nothing to run against"));
    };
    let update = bijective_serialization::signal::market::decode(&entry.payload)
        .map_err(|e| format!("{market} position 0 is unreadable: {e:?}"))?;
    Ok(update.received_at_ns.unwrap_or(i128::from(update.venue_ts_ms) * 1_000_000))
}

/// The configuration the order path runs under: the flag, or the project's `config`.
pub(crate) fn configuration(flag: Option<PathBuf>, project: Option<&str>) -> Option<PathBuf> {
    flag.or_else(|| project.map(PathBuf::from))
}
