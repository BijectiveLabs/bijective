// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What two runs decided, and where they differ.
//! The paper-to-live comparison of the promotion stages, over decisions rather than instants.

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;

use bijective_control_engine::wire::{ControlEvent, RefusalReason, decode};
use bijective_event_store::store::{StoreError, read_range};
use bijective_model::OrderRequest;

/// What a run did about one proposal. Two answers only: gate 35 asserts the order path answers every proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// It formed an order, and this is the order it formed.
    Intended(OrderRequest),
    /// It refused, and this is why.
    Refused(RefusalReason),
}

/// Which run held a decision the other did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Paper,
    Live,
}

/// One disagreement, at the proposal that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// Both answered, and differently. Boxed: a decision carries a whole order, and a diverged-everywhere run has the longest list.
    DecidedDifferently { at: u64, decided: Box<(Decision, Decision)> },
    /// One answered and the other did not. Kept as a divergence: dropping positions only one reached would report agreement on what was missing.
    OnlyOneAnswered { at: u64, answered_by: Side },
}

/// Why a comparison could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotComparable {
    /// Neither run decided anything.
    NeitherDecided,
    /// No proposal in common: two different proposal streams, not a strategy disagreement.
    NoSharedProposal { paper: usize, live: usize },
}

/// What two runs say about each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    /// Every shared position decided the same way.
    Agreed {
        positions: usize,
    },
    Diverged {
        compared: usize,
        divergences: Vec<Divergence>,
    },
    /// **Not agreement.** Two runs that decided nothing agree, and the agreement means nothing.
    NotComparable(NotComparable),
}

impl Comparison {
    /// Only `Agreed` counts: *not comparable* means unknown and must never read as a yes.
    #[must_use]
    pub fn agreed(&self) -> bool {
        matches!(self, Comparison::Agreed { .. })
    }
}

impl core::fmt::Display for NotComparable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NotComparable::NeitherDecided => write!(f, "neither run decided anything"),
            NotComparable::NoSharedProposal { paper, live } => write!(
                f,
                "no proposal in common: {paper} decided in paper, {live} live, none the same"
            ),
        }
    }
}

impl core::fmt::Display for Comparison {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Comparison::Agreed { positions } => {
                write!(f, "agreed on {positions} proposal(s)")
            }
            Comparison::Diverged { compared, divergences } => write!(
                f,
                "{} divergence(s) over {compared} proposal(s) compared",
                divergences.len()
            ),
            Comparison::NotComparable(why) => write!(f, "not comparable: {why}"),
        }
    }
}

#[derive(Debug)]
pub enum ReadError {
    Store(StoreError),
    Undecodable { at: u64 },
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReadError::Store(e) => write!(f, "store: {e:?}"),
            ReadError::Undecodable { at } => {
                write!(f, "the control entry at position {at} could not be read")
            }
        }
    }
}

impl std::error::Error for ReadError {}

/// Read what a run decided, keyed on the proposal. An unreadable entry is refused, never skipped: it may be the decision under comparison.
pub fn decisions(root: &Path, stream: &str) -> Result<BTreeMap<u64, Decision>, ReadError> {
    let entries = read_range(root, stream, 0, u64::MAX).map_err(ReadError::Store)?;

    let mut out = BTreeMap::new();
    for entry in entries {
        let event = decode(&entry.payload).map_err(|_| ReadError::Undecodable { at: entry.seq })?;
        match event {
            ControlEvent::Intent { request, from_position, .. } => {
                out.insert(from_position, Decision::Intended(request));
            }
            ControlEvent::Refused { reason, from_position, .. } => {
                out.insert(from_position, Decision::Refused(reason));
            }
            // Settlements and instants differ by design between runs. Comparing them would flag every record.
            _ => {}
        }
    }

    Ok(out)
}

/// Compare what two runs decided.
#[must_use]
pub fn compare(paper: &BTreeMap<u64, Decision>, live: &BTreeMap<u64, Decision>) -> Comparison {
    if paper.is_empty() && live.is_empty() {
        return Comparison::NotComparable(NotComparable::NeitherDecided);
    }

    let shared = paper.keys().filter(|k| live.contains_key(k)).count();
    if shared == 0 {
        return Comparison::NotComparable(NotComparable::NoSharedProposal {
            paper: paper.len(),
            live: live.len(),
        });
    }

    let mut divergences = Vec::new();
    let mut positions: Vec<u64> = paper.keys().chain(live.keys()).copied().collect();
    positions.sort_unstable();
    positions.dedup();

    for at in positions {
        match (paper.get(&at), live.get(&at)) {
            (Some(s), Some(l)) if s == l => {}
            (Some(s), Some(l)) => divergences.push(Divergence::DecidedDifferently {
                at,
                decided: Box::new((s.clone(), l.clone())),
            }),
            (Some(_), None) => {
                divergences.push(Divergence::OnlyOneAnswered { at, answered_by: Side::Paper });
            }
            (None, Some(_)) => {
                divergences.push(Divergence::OnlyOneAnswered { at, answered_by: Side::Live });
            }
            (None, None) => unreachable!("the position came from one of the two maps"),
        }
    }

    if divergences.is_empty() {
        Comparison::Agreed { positions: shared }
    } else {
        Comparison::Diverged { compared: shared, divergences }
    }
}
