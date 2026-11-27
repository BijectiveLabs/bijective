// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Whether a strategy may take the next step towards capital.
//! Design: the promotion stages. The decision is a value, not an act (`docs/concepts/evaluation.md`).

pub mod path;
pub mod stage;
pub mod steps;

pub use stage::{Backtest, Cleared, Live, Paper, Stage, StageError};

#[cfg(test)]
#[path = "../../tests/unit/promotion/tests.rs"]
mod tests;

use crate::evidence::{Evidence, Outcome, Verdict, WalkForward};
use bijective_event_store::digest::Digest;
use bijective_strategy_engine::Reproduction;
use bijective_strategy_engine::runs::ledger::RunLedgerEntry;

/// Why a promotion was refused: one variant per term, each carrying what the term said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    DidNotReproduce {
        first: Digest,
        again: Digest,
    },
    /// Not replayed, so nothing was compared. Distinct from a divergence: an unrepeated run must not pass as repeated.
    NotReproduced,
    OutputUnusable {
        exit: String,
    },
    ClaimNotMet {
        outcome: Outcome,
    },
    InSampleOnly,
    /// Nothing measured the run. Not the same as a claim that was not met. Ordinary today, since
    /// nothing yet turns a run's output into figures.
    NotMeasured,
    EvidenceShort {
        evidence: Evidence,
    },
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refusal::DidNotReproduce { .. } => {
                write!(f, "the run did not reproduce, so its verdict cannot be stood behind")
            }
            Refusal::NotReproduced => {
                write!(f, "the run was never replayed, so nothing was compared")
            }
            Refusal::OutputUnusable { exit } => {
                write!(f, "the run was cut short ({exit}), so what it emitted is not a result")
            }
            Refusal::ClaimNotMet { outcome } => write!(f, "the claim was not met: {outcome:?}"),
            Refusal::InSampleOnly => {
                write!(f, "the result was measured only where it was fitted")
            }
            Refusal::NotMeasured => {
                write!(f, "nothing measured this run, so its claim has not been answered")
            }
            Refusal::EvidenceShort { evidence } => write!(f, "{evidence}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// What a promotion decision was made from. Every field is produced elsewhere. A promotion needs every term to be yes.
#[derive(Debug, Clone)]
pub struct Presented<'a> {
    pub entry: &'a RunLedgerEntry,
    pub reproduction: Reproduction,
    /// `None` means this run was not measured, which is not a failed claim.
    pub verdict: Option<&'a Verdict>,
    pub held_out: Option<&'a WalkForward>,
}

/// Assess every term and return the first that is not yes. Reproduction is asked first, since an
/// unrepeatable run makes every later term a statement about one event. Returned, never acted on.
pub fn assess(presented: &Presented<'_>) -> Result<Promoted, Refusal> {
    match presented.reproduction {
        Reproduction::Diverged { first, again } => {
            return Err(Refusal::DidNotReproduce { first, again });
        }
        Reproduction::NotCompared => return Err(Refusal::NotReproduced),
        // A class that claims no replay is not excused a term. A live session rests on the bounded run it follows.
        Reproduction::Identical | Reproduction::NotClaimed => {}
    }

    if !presented.entry.exit.output_is_usable() {
        return Err(Refusal::OutputUnusable { exit: format!("{:?}", presented.entry.exit) });
    }

    // Asked after the run's own terms and before the claim's, so an unreproduced run is refused for reproduction first.
    let Some(verdict) = presented.verdict else {
        return Err(Refusal::NotMeasured);
    };

    if !verdict.outcome.supports_promotion() {
        return Err(Refusal::ClaimNotMet { outcome: verdict.outcome.clone() });
    }

    let Some(held_out) = presented.held_out else {
        return Err(Refusal::InSampleOnly);
    };

    if !verdict.evidence.supports_promotion() {
        return Err(Refusal::EvidenceShort { evidence: verdict.evidence });
    }

    Ok(Promoted {
        run: presented.entry.specification,
        strategy: presented.entry.strategy,
        content: presented.entry.content,
        held_out_positions: held_out.held_out_positions(),
    })
}

/// Every term was yes. Carries its run so a stage cannot be handed one decision for another run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Promoted {
    pub run: Digest,
    /// `None` is a run recorded before the ledger could name a strategy. A stage cannot be cleared on one.
    pub strategy: Option<Digest>,
    /// A stage cannot be cleared for one version and carried to another. `None` is a run
    /// recorded before the ledger could say which bytes ran.
    pub content: Option<Digest>,
    pub held_out_positions: u64,
}
