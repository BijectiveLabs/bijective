// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The streams a control engine reads and writes, named once for every program that meets them.
//!
//! A paper node keeps a record apart from a live one: its own proposals, control, limit and command
//! streams. A paper session's proposals, an operator's paper command and the feedback a paper
//! strategy is told can then never reach, resume or be mistaken for the live node's, even on one
//! store.

/// One node's streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeStreams {
    /// The run mode these streams belong to, as every program spells it: `live`, `paper` or
    /// `backtest`.
    pub mode: &'static str,
    /// What strategies propose, and the node judges.
    pub proposals: &'static str,
    /// The node's record of every decision. Its feedback mirror is named after it.
    pub control: &'static str,
    /// Limit declarations, consumption and releases.
    pub limits: &'static str,
    /// Signed operator commands.
    pub commands: &'static str,
}

/// The live node's, which places capital.
pub const LIVE: NodeStreams = NodeStreams {
    mode: "live",
    proposals: "proposals",
    control: "control",
    limits: "limits",
    commands: "commands",
};

/// The paper node's, which matches on the simulated venue.
pub const PAPER: NodeStreams = NodeStreams {
    mode: "paper",
    proposals: "proposals-paper",
    control: "control-paper",
    limits: "limits-paper",
    commands: "commands-paper",
};

/// A backtest's, in a record directory of its own (decision 0015): a backtest pointed at a node's
/// store still writes nothing that node reads.
pub const BACKTEST: NodeStreams = NodeStreams {
    mode: "backtest",
    proposals: "proposals-backtest",
    control: "control-backtest",
    limits: "limits-backtest",
    commands: "commands-backtest",
};

/// The streams of the mode named `mode`, or `None` for a name no program uses.
#[must_use]
pub fn of_mode(mode: &str) -> Option<NodeStreams> {
    [LIVE, PAPER, BACKTEST].into_iter().find(|streams| streams.mode == mode)
}
