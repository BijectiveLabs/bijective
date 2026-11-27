// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Where a backtest's passes fall in a recording (`docs/decisions/0015-backtest-through-the-order-path.md`).
//!
//! A pass runs at a position where a proposal was decided, once a heartbeat of recorded time has
//! passed since the last pass, and at the window's last position. Pure, so the schedule is tested
//! without a store.

/// One entry of the driving market stream: its position and the instant it was received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketEntry {
    pub position: u64,
    pub at_ns: i128,
}

/// One proposal: its position on the proposal stream and the market position it was decided at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decided {
    pub seq: u64,
    pub at_position: u64,
}

/// One pass: the market read through `position`, the clock at `at_ns`, and the proposal stream
/// read through `proposals_through`, which is `None` while no proposal is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub position: u64,
    pub at_ns: i128,
    pub proposals_through: Option<u64>,
}

/// The passes, and how many proposals were decided past the window's last position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub steps: Vec<Step>,
    pub unreached: usize,
}

/// The passes over `market`, in stream order. The clock never moves back: an entry received before
/// the one ahead of it is stepped at the later instant. A proposal is due once it and every
/// proposal before it on the stream were decided at or before the step's position, so none is
/// judged ahead of a predecessor.
#[must_use]
pub fn schedule(market: &[MarketEntry], decided: &[Decided], heartbeat_ns: i128) -> Schedule {
    let mut steps = Vec::new();
    let mut due = 0usize;
    let mut last_pass_ns: Option<i128> = None;
    let mut clock_ns = i128::MIN;
    for (index, entry) in market.iter().enumerate() {
        clock_ns = clock_ns.max(entry.at_ns);
        let before = due;
        while decided.get(due).is_some_and(|d| d.at_position <= entry.position) {
            due += 1;
        }
        let beat = last_pass_ns.is_none_or(|at| clock_ns - at >= heartbeat_ns);
        if due > before || beat || index + 1 == market.len() {
            let proposals_through = due.checked_sub(1).map(|last| decided[last].seq);
            steps.push(Step { position: entry.position, at_ns: clock_ns, proposals_through });
            last_pass_ns = Some(clock_ns);
        }
    }
    Schedule { steps, unreached: decided.len() - due }
}

#[cfg(test)]
#[path = "../tests/unit/backtest_tests.rs"]
mod tests;
