// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The Control Engine's startup decision, separated from the process that takes it.
//! Drivable without a store on disk.

pub mod backtest;
pub mod config;
pub mod daily;
pub mod feedback;
pub mod flatten;
pub mod linked;
pub mod loop_;
pub mod loss;
pub mod operators;
pub mod order_id;
pub mod orders;
pub mod path;
pub mod progress;
pub mod rate;
pub mod release;
pub mod replay;
pub mod run_mode;
pub mod summary;
pub mod sweep;
pub mod trading;
pub mod unverified;
pub mod wire;

pub use config::{Accepted, Configuration};
pub use loop_::{Acted, Carried, Judgement, ProposalWindow, Reachable, carry_out_commands, drive};
pub use operators::parse_operators;
pub use path::{Benchmark, Decision, OrderPath, PathError, Placement, Reconciled, Spend};
pub use progress::{Progress, ProgressError};
pub use replay::{ReplayError, Replayed};
pub use run_mode::{Inputs, PlanError, RunMode, RunPlan};
pub use sweep::{Resting, Swept, resting_to_withdraw, withdraw_resting};
pub use trading::TradingState;
pub use unverified::{Unverified, UnverifiedError};
pub use wire::{ControlEvent, RefusalReason};

use bijective_limits::{Evaluation, Freshness};
use bijective_model::Clearance;
use bijective_risk::limit_state::Recovery;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    /// The program is up. Trading stays halted regardless: nothing here sets it.
    Up {
        detail: String,
    },
    Refused {
        detail: String,
    },
}

impl Startup {
    pub fn is_up(&self) -> bool {
        matches!(self, Self::Up { .. })
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Up { detail } | Self::Refused { detail } => detail,
        }
    }
}

/// A verified but empty history is up: first boot is not the same as a failed replay.
pub fn assess(recovery: Recovery) -> Startup {
    match recovery {
        Recovery::Verified { through: Some(position) } => Startup::Up {
            detail: format!("limit state verified through position {position}; trading halted"),
        },
        Recovery::Verified { through: None } => Startup::Up {
            detail: "limit state verified over an empty history; trading halted".to_string(),
        },
        Recovery::Incomplete => Startup::Refused {
            detail: "replay did not complete, so no limit is known to be in force".to_string(),
        },
    }
}

/// Turn a risk evaluation into the order-path key. The only place that mints one.
#[must_use]
pub fn clear_to_submit(evaluation: &Evaluation, at_ns: i128) -> Option<Clearance> {
    // Ask the evaluation itself so the domain's own rule decides, not a restated copy of it.
    if !evaluation.is_clear() {
        return None;
    }

    Clearance::granted(
        evaluation.breaches.len(),
        evaluation.unjudged.len(),
        matches!(evaluation.freshness, Freshness::Fresh),
        at_ns,
    )
}

#[cfg(test)]
#[path = "../tests/unit/adjust_tests.rs"]
mod adjust_tests;
#[cfg(test)]
#[path = "../tests/unit/fills_tests.rs"]
mod fills_tests;
#[cfg(test)]
#[path = "../tests/unit/order_id_tests.rs"]
mod order_id_tests;
#[cfg(test)]
#[path = "../tests/unit/path_tests.rs"]
mod path_tests;
#[cfg(test)]
#[path = "../tests/unit/reconcile_tests.rs"]
mod reconcile_tests;
#[cfg(test)]
#[path = "../tests/unit/reservations_tests.rs"]
mod reservations_tests;
#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
