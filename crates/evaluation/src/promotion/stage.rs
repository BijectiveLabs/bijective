// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Backtest, then paper, then live, with no way to write the sequence out of order.
//! Skipping is unsayable rather than refused: a stage takes the prior stage's output, and no
//! constructor takes nothing.

use bijective_event_store::digest::Digest;

use crate::promotion::Promoted;

pub trait Stage {
    /// What must already have been decided: the first stage requires nothing, every later one its predecessor.
    type Requires;

    const NAME: &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backtest;

impl Stage for Backtest {
    type Requires = ();
    const NAME: &'static str = "backtest";
}

/// Judged against live market data. Proposals go to a stream the Control Engine does not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paper;

impl Stage for Paper {
    type Requires = Cleared<Backtest>;
    const NAME: &'static str = "paper";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live;

impl Stage for Live {
    type Requires = Cleared<Paper>;
    const NAME: &'static str = "live";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageError {
    /// The prior stage was cleared for a different strategy. Not a different run: each stage is a
    /// different run by construction, so what must match is program, arguments and environment.
    PriorStageWasAnotherStrategy { prior: Digest, this: Digest },
    /// Refused rather than passed: a stage cleared for a nameless strategy clears whatever follows.
    StrategyNotRecorded { run: Digest },
    /// The prior stage cleared a different version. Reproduction cannot catch it, since
    /// two versions emitting the same bytes reproduce identically.
    PriorStageRanOtherBytes { prior: Digest, this: Digest },
    /// Refused like an unrecorded strategy: a stage that cannot say which version it cleared clears every later one.
    BytesNotRecorded { run: Digest },
}

impl core::fmt::Display for StageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StageError::PriorStageWasAnotherStrategy { .. } => write!(
                f,
                "the prior stage was cleared for a different strategy, so this one has no predecessor"
            ),
            StageError::StrategyNotRecorded { .. } => write!(
                f,
                "the ledger does not say which strategy this run was of, so no stage can rest on it"
            ),
            StageError::PriorStageRanOtherBytes { .. } => write!(
                f,
                "the prior stage cleared a different version of this program, so its clearance does \
                 not carry"
            ),
            StageError::BytesNotRecorded { .. } => write!(
                f,
                "the ledger does not say which version of the program ran, so no stage can rest on it"
            ),
        }
    }
}

impl std::error::Error for StageError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleared<S: Stage> {
    decision: Promoted,
    stage: core::marker::PhantomData<S>,
}

impl<S: Stage> Cleared<S> {
    #[must_use]
    pub fn run(&self) -> Digest {
        self.decision.run
    }

    #[must_use]
    pub fn strategy(&self) -> Option<Digest> {
        self.decision.strategy
    }

    #[must_use]
    pub fn content(&self) -> Option<Digest> {
        self.decision.content
    }

    #[must_use]
    pub fn held_out_positions(&self) -> u64 {
        self.decision.held_out_positions
    }

    #[must_use]
    pub fn stage(&self) -> &'static str {
        S::NAME
    }
}

impl Cleared<Backtest> {
    /// Still requires a strategy: a first stage for a nameless one would clear every later stage.
    pub fn first(decision: Promoted) -> Result<Self, StageError> {
        if decision.strategy.is_none() {
            return Err(StageError::StrategyNotRecorded { run: decision.run });
        }
        Ok(Self { decision, stage: core::marker::PhantomData })
    }
}

impl Cleared<Paper> {
    pub fn after(prior: Cleared<Backtest>, decision: Promoted) -> Result<Self, StageError> {
        same_strategy(prior.strategy(), decision)?;
        same_bytes(prior.content(), decision)?;
        Ok(Self { decision, stage: core::marker::PhantomData })
    }
}

impl Cleared<Live> {
    pub fn after(prior: Cleared<Paper>, decision: Promoted) -> Result<Self, StageError> {
        same_strategy(prior.strategy(), decision)?;
        same_bytes(prior.content(), decision)?;
        Ok(Self { decision, stage: core::marker::PhantomData })
    }
}

/// Between stages, not within one: a clearance for one version must not be spent on another.
fn same_bytes(prior: Option<Digest>, decision: Promoted) -> Result<(), StageError> {
    let (Some(prior), Some(this)) = (prior, decision.content) else {
        return Err(StageError::BytesNotRecorded { run: decision.run });
    };
    if prior == this { Ok(()) } else { Err(StageError::PriorStageRanOtherBytes { prior, this }) }
}

fn same_strategy(prior: Option<Digest>, decision: Promoted) -> Result<(), StageError> {
    let (Some(prior), Some(this)) = (prior, decision.strategy) else {
        return Err(StageError::StrategyNotRecorded { run: decision.run });
    };
    if prior == this {
        Ok(())
    } else {
        Err(StageError::PriorStageWasAnotherStrategy { prior, this })
    }
}
