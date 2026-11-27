// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a run is for and where its inputs come from. Fixed before the clock is first read.
//!
//! Two questions with separate answers. The mode is the run's purpose, in the order a strategy is
//! promoted: backtest, paper, live. The inputs are where events and time come from: the live
//! feed and the wall clock, or a recorded sequence and a clock driven by it. Replay is the second
//! answer, not a mode: every decision is journaled so that it can be re-derived from its inputs.

use bijective_common::clock::{Clock, SequenceClock, WallClock};

/// What the run is for, in the order a strategy is promoted. Only a live run may send an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// A strategy's proposals and the order path over recorded history, on simulated venues
    /// only (decision 0015).
    Backtest,
    /// Live data and the wall clock. Orders are evaluated and recorded but never sent.
    Paper,
    /// Live data and the wall clock. Orders reach the venue. Capital at risk.
    Live,
}

/// Where events and time come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inputs {
    /// The live feed and the wall clock.
    Live,
    /// A recorded sequence, with a clock that starts at `from_ns` and moves only with the sequence.
    Recorded { from_ns: i128 },
}

/// A mode on inputs it is allowed to run on. `select` is the only way to make one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunPlan {
    mode: RunMode,
    inputs: Inputs,
}

/// Why a command line names no valid plan. Each message says what to pass instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    ModeMissing,
    UnknownMode(String),
    BacktestNeedsRecordedInputs,
    ReplayIsOnlyBuiltForBacktest { mode: String },
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModeMissing => {
                write!(f, "--mode is required: backtest, paper or live; no mode is assumed")
            }
            Self::UnknownMode(name) => write!(
                f,
                "--mode is backtest, paper or live, not {name:?}; replay is not a mode, it is \
                 how a backtest is fed (--replay-from)"
            ),
            Self::BacktestNeedsRecordedInputs => write!(
                f,
                "--mode backtest runs over recorded history: give the instant it starts at with \
                 --replay-from <ns>; over live data the run is --mode paper"
            ),
            Self::ReplayIsOnlyBuiltForBacktest { mode } => write!(
                f,
                "--replay-from belongs to --mode backtest; replaying a {mode} run on its journal \
                 is not built, and a {mode} run is never started on a clock it was not given"
            ),
        }
    }
}

impl std::error::Error for PlanError {}

impl RunPlan {
    /// The one place the rules are written. Adding replay for paper or live relaxes an arm here
    /// and nothing else. `mode` is required: the former default was live, which reaches venues.
    pub fn select(mode: Option<&str>, replay_from: Option<i128>) -> Result<Self, PlanError> {
        let name = mode.ok_or(PlanError::ModeMissing)?;
        let mode = match name {
            "backtest" => RunMode::Backtest,
            "paper" => RunMode::Paper,
            "live" => RunMode::Live,
            other => return Err(PlanError::UnknownMode(other.to_string())),
        };
        let inputs = match (mode, replay_from) {
            (RunMode::Backtest, Some(from_ns)) => Inputs::Recorded { from_ns },
            (RunMode::Backtest, None) => return Err(PlanError::BacktestNeedsRecordedInputs),
            (_, Some(_)) => {
                return Err(PlanError::ReplayIsOnlyBuiltForBacktest { mode: name.to_string() });
            }
            (_, None) => Inputs::Live,
        };
        Ok(Self { mode, inputs })
    }

    #[must_use]
    pub fn mode(self) -> RunMode {
        self.mode
    }

    #[must_use]
    pub fn inputs(self) -> Inputs {
        self.inputs
    }

    /// The clock the program reads for the whole run.
    #[must_use]
    pub fn clock(self) -> Box<dyn Clock> {
        match self.inputs {
            Inputs::Live => Box::new(WallClock::new()),
            Inputs::Recorded { from_ns } => Box::new(SequenceClock::starting_at(from_ns)),
        }
    }

    /// Orders reach a venue only in a live mode on live inputs. Every other combination is false.
    #[must_use]
    pub fn reaches_venues(self) -> bool {
        self.mode == RunMode::Live && self.inputs == Inputs::Live
    }

    /// The mode's name as the command line and the readiness detail spell it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self.mode {
            RunMode::Backtest => "backtest",
            RunMode::Paper => "paper",
            RunMode::Live => "live",
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/run_mode_tests.rs"]
mod tests;
