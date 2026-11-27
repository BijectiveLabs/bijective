// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Supervises one agent run: may it happen at all, and is what it produced usable (research integrity to A5).

use std::time::Duration;

pub mod extract;
pub mod launch;

pub mod ledger;
mod pump;
pub mod session;

pub use extract::{ExtractError, materialise};
pub use pump::{
    Admitting, Fed, PumpError, Refused, admit_lines, admit_result, is_facts, market, opening,
    read_fed, read_fed_after, since, window,
};

use crate::runs::{Authority, ExitReason, RunLedgerEntry, RunSpec};
use bijective_event_store::digest::{Digest, sha256};
use bijective_sandbox::{Completed, Exit, Missing};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    Ready {
        detail: String,
    },
    /// The host cannot bound a run. No degraded mode: an unisolated run yields a verdict that looks like any other.
    Refused {
        detail: String,
    },
}

impl Startup {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Ready { detail } | Self::Refused { detail } => detail,
        }
    }
}

#[must_use]
pub fn assess(probe: Result<(), Missing>) -> Startup {
    match probe {
        Ok(()) => Startup::Ready { detail: "agent runs can be bounded; none started".to_string() },
        Err(Missing::UnshareAbsent) => Startup::Refused {
            detail: "unshare(1) is not on this host, so no run can be isolated".to_string(),
        },
        Err(Missing::UserNamespaces) => Startup::Refused {
            detail: "unprivileged user namespaces are disabled on this host".to_string(),
        },
        Err(Missing::CgroupSubtree { detail }) => Startup::Refused {
            detail: format!("no cgroup v2 subtree this process may write to: {detail}"),
        },
    }
}

/// One translation of exit to ledger reason: both lifetimes record an exit and two mappings would drift.
#[must_use]
pub fn reason_of(exit: &Exit) -> ExitReason {
    match exit {
        Exit::Completed { status } => ExitReason::Completed { status: *status },
        Exit::LimitExceeded => ExitReason::LimitExceeded,
        Exit::Timeout => ExitReason::Timeout,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supervised {
    pub entry: RunLedgerEntry,
    /// Only where the entry says the output may be used: a cut-short pass read as a result is how an underpowered verdict becomes a promotion.
    pub output: Option<Vec<u8>>,
}

/// The entry is produced first and unconditionally: a crash after emitting must be distinguishable from a run that never happened.
#[must_use]
pub fn supervise<A: Authority>(
    spec: &RunSpec<A>,
    completed: &Completed,
    started_ns: i128,
    ended_ns: i128,
) -> Supervised {
    // Hashed here, not passed in: the bytes that matter are those on disk when the run was supervised.
    let content = std::fs::read(spec.program()).ok().map(|bytes| sha256(&bytes));
    let exit = reason_of(&completed.exit);

    let entry = RunLedgerEntry {
        specification: spec.identity(),
        class: spec.class().to_string(),
        started_ns,
        ended_ns,
        exit,
        emitted: sha256(&completed.stdout),
        strategy: Some(spec.strategy()),
        // Carried so the record can name the strategy. A digest cannot be turned back into a name.
        program: Some(spec.program().to_string()),
        content,
    };

    let output = exit.output_is_usable().then(|| completed.stdout.clone());

    Supervised { entry, output }
}

/// Whether a replay reproduced the run it repeated.
/// Not a failed run, but one whose verdict cannot be stood behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reproduction {
    Identical,
    Diverged {
        first: Digest,
        again: Digest,
    },
    /// The replay did not complete, so nothing was compared.
    NotCompared,
    /// Not a pass like `Identical`: a session reading the present cannot be replayed.
    NotClaimed,
}

#[must_use]
pub fn compare(first: &Supervised, again: &Supervised) -> Reproduction {
    reproduction_of(&first.entry.class.clone(), &[first.entry.clone(), again.entry.clone()])
}

/// The same rule as `compare`, over ledger entries. A single run is `NotCompared`: there is nothing to compare.
/// Uses the two most recent usable entries: the comparison is against the last run that could be stood behind.
#[must_use]
pub fn reproduction_of(class: &str, entries: &[RunLedgerEntry]) -> Reproduction {
    // A class this build cannot name is asked the stricter question rather than excused from it.
    if crate::runs::replays(class) == Some(false) {
        return Reproduction::NotClaimed;
    }

    let mut usable = entries.iter().filter(|e| e.exit.output_is_usable()).rev();
    let (Some(again), Some(first)) = (usable.next(), usable.next()) else {
        return Reproduction::NotCompared;
    };

    if first.emitted == again.emitted {
        Reproduction::Identical
    } else {
        Reproduction::Diverged { first: first.emitted, again: again.emitted }
    }
}

#[must_use]
/// How far behind a strategy may fall before its session ends, and whether anyone chose it.
/// Printed differently when declared: an unset bound and one set to the same number are different facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behind {
    Declared(usize),
    /// What the platform absorbs, measured. Not a policy - see `QUEUED_BY_DEFAULT`.
    NotDeclared(usize),
}

impl Behind {
    #[must_use]
    pub fn events(self) -> usize {
        match self {
            Behind::Declared(n) | Behind::NotDeclared(n) => n,
        }
    }
}

impl core::fmt::Display for Behind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Behind::Declared(n) => write!(f, "{n} event(s) behind, declared"),
            Behind::NotDeclared(n) => write!(f, "{n} event(s) behind, a bound not declared"),
        }
    }
}

/// Deliberately small: every run inherits the default, so a generous one shows up as memory pressure.
pub fn conservative_wall() -> Duration {
    Duration::from_secs(300)
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

impl core::fmt::Display for Reproduction {
    /// Both digests: "these two differ" is where an operator starts.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Reproduction::Identical => write!(f, "identical"),
            Reproduction::Diverged { first, again } => {
                write!(f, "diverged: {} then {}", short_digest(first), short_digest(again))
            }
            Reproduction::NotCompared => write!(f, "not compared"),
            Reproduction::NotClaimed => write!(f, "not claimed by this class"),
        }
    }
}

fn short_digest(digest: &bijective_event_store::digest::Digest) -> String {
    digest.0[..8].iter().map(|b| format!("{b:02x}")).collect()
}

pub mod protocol;
pub mod runs;
