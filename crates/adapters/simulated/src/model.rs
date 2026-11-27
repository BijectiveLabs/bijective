// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The execution assumptions a simulated run makes, declared as data so every result states them.

/// When a resting limit order fills against the recorded book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillModel {
    /// Only when the opposite side trades through the limit. A resting order's place in the queue is
    /// unknown, so touching its price is not evidence it would have filled. The default.
    TradeThrough,
    /// When the opposite side reaches the limit. Optimistic: assumes the order was first in queue.
    Touch,
}

/// How long a command takes to reach the matching engine, in nanoseconds of the run's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyModel {
    pub submit_ns: i128,
    pub cancel_ns: i128,
}

/// Raised whenever the matching rules change, so a result names the rules that produced it.
pub const MATCHING_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulationModel {
    pub fill: FillModel,
    pub latency: LatencyModel,
}

impl SimulationModel {
    /// One sentence for a report: what this run assumed about execution.
    pub fn describe(&self) -> String {
        format!(
            "matching v{MATCHING_VERSION}, {:?} fills, submit latency {} ns, cancel latency {} ns, no fees charged",
            self.fill, self.latency.submit_ns, self.latency.cancel_ns
        )
    }
}
