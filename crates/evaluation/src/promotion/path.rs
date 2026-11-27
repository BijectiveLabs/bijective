// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One path from the platform's own record to a promotion decision. the promotion stages section 3.1.
//! Nothing here acts. The decision is a value.

use std::collections::BTreeMap;
use std::path::Path;

use crate::evidence::registry::{self, REGISTRATIONS};
use crate::evidence::{Evidence, Measured, PreRegistration, judge};
use bijective_event_store::digest::Digest;
use bijective_event_store::projection::Recovered;
use bijective_strategy_engine::ledger::{Ledger, RUNS};
use bijective_strategy_engine::runs::ledger::{Admitted, RunLedgerEntry};
use bijective_strategy_engine::{Reproduction, reproduction_of};

use crate::promotion::stage::{Backtest, Cleared, Paper, StageError};
use crate::promotion::{Presented, Promoted, Refusal};

#[derive(Debug)]
pub enum GatherError {
    Ledger(bijective_strategy_engine::ledger::LedgerError),
    Rebuild(String),
    Registry(registry::RegistryError),
}

impl core::fmt::Display for GatherError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GatherError::Ledger(e) => write!(f, "the run ledger could not be read: {e}"),
            GatherError::Rebuild(e) => write!(f, "the run ledger could not be rebuilt: {e}"),
            GatherError::Registry(e) => write!(f, "the registry could not be read: {e:?}"),
        }
    }
}

impl std::error::Error for GatherError {}

/// What the platform holds about one run, keyed by identity so a decision cannot pair one run's
/// result with another's registration.
#[derive(Debug, Clone)]
pub struct Gathered {
    pub run: Digest,
    pub entry: RunLedgerEntry,
    pub entries: Vec<RunLedgerEntry>,
    pub reproduction: Reproduction,
    pub registration: Option<PreRegistration>,
}

/// `NotMeasured` is not an empty list: that would read as measured and found nothing.
#[derive(Debug, Clone)]
pub enum Measurement<'a> {
    Measured { measured: &'a [Measured], evidence: Evidence },
    NotMeasured,
}

/// A run with no ledger entry is `None`, not an empty decision: it did not happen.
pub fn gather(root: &Path, run: Digest) -> Result<Option<Gathered>, GatherError> {
    let held = Recovered::<Ledger>::rebuild(root, RUNS)
        .map_err(|e| GatherError::Rebuild(format!("{e:?}")))?;
    let Some(ledger) = held.state() else {
        return Err(GatherError::Rebuild("the rebuild did not complete".to_string()));
    };

    let entries = ledger.of(&run).to_vec();
    let Some(entry) = entries.last().cloned() else {
        return Ok(None);
    };

    let registration = registry::standing(root, REGISTRATIONS, run)
        .map_err(GatherError::Registry)?
        .as_ref()
        .map(PreRegistration::from);

    Ok(Some(Gathered {
        run,
        reproduction: reproduction_of(&entry.class, &entries),
        entry,
        entries,
        registration,
    }))
}

/// Every run this node has recorded, newest last, so an operator need not type a 64-character digest.
pub fn identities(root: &Path) -> Result<Vec<Digest>, GatherError> {
    let held = Recovered::<Ledger>::rebuild(root, RUNS)
        .map_err(|e| GatherError::Rebuild(format!("{e:?}")))?;
    held.state()
        .map(Ledger::identities)
        .ok_or_else(|| GatherError::Rebuild("the rebuild did not complete".to_string()))
}

/// The effect is the claim's own threshold, never a figure chosen beside it. The confidence is
/// the operator's declared critical value and is never defaulted.
#[must_use]
pub fn evidence_of(
    gathered: &Gathered,
    measurement: &crate::measure::Measurement,
    confidence: i128,
) -> Evidence {
    let effect = gathered.registration.as_ref().map_or(0, |r| r.rule().threshold);
    crate::evidence::assess(measurement.observations, confidence, measurement.dispersion, effect)
}

/// Every strategy this node has recorded, newest first. One ledger rebuild, not one per run.
/// A nameless strategy is listed under its digest rather than an invented name.
pub fn every_strategy(root: &Path) -> Result<Vec<Recorded>, GatherError> {
    let held = Recovered::<Ledger>::rebuild(root, RUNS)
        .map_err(|e| GatherError::Rebuild(format!("{e:?}")))?;
    let Some(ledger) = held.state() else {
        return Err(GatherError::Rebuild("the rebuild did not complete".to_string()));
    };

    let mut found: BTreeMap<String, Recorded> = BTreeMap::new();
    for identity in ledger.identities() {
        for entry in ledger.of(&identity) {
            let Some(strategy) = entry.strategy else {
                continue;
            };
            let held = found.entry(strategy.to_hex()).or_insert_with(|| Recorded {
                strategy,
                program: None,
                runs: 0,
                classes: Vec::new(),
                last_touched_ns: i128::MIN,
            });
            held.runs += 1;
            held.last_touched_ns = held.last_touched_ns.max(entry.started_ns);
            if held.program.is_none() {
                held.program.clone_from(&entry.program);
            }
            if !held.classes.iter().any(|c| c == &entry.class) {
                held.classes.push(entry.class.clone());
            }
        }
    }

    let mut listed: Vec<Recorded> = found.into_values().collect();
    listed.sort_by_key(|one| std::cmp::Reverse(one.last_touched_ns));
    Ok(listed)
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub strategy: Digest,
    pub program: Option<String>,
    pub runs: u64,
    pub classes: Vec<String>,
    pub last_touched_ns: i128,
}

/// Where one run's proposals landed. The stream, not the ledger class, says whether they could
/// reach a venue, so a paper figure cannot be read as a live one.
pub fn landed(root: &Path, run: Digest) -> Result<Vec<Admitted>, GatherError> {
    let held = Recovered::<Ledger>::rebuild(root, RUNS)
        .map_err(|e| GatherError::Rebuild(format!("{e:?}")))?;
    held.state()
        .map(|ledger| ledger.admissions_of(&run).to_vec())
        .ok_or_else(|| GatherError::Rebuild("the rebuild did not complete".to_string()))
}

pub fn runs_of(root: &Path, strategy: Digest) -> Result<Vec<(Digest, String)>, GatherError> {
    let held = Recovered::<Ledger>::rebuild(root, RUNS)
        .map_err(|e| GatherError::Rebuild(format!("{e:?}")))?;
    held.state()
        .map(|ledger| ledger.of_strategy(&strategy))
        .ok_or_else(|| GatherError::Rebuild("the rebuild did not complete".to_string()))
}

/// The claim is judged against the identity the ledger holds, never the one the registration names.
pub fn decide(gathered: &Gathered, measurement: &Measurement<'_>) -> Result<Promoted, Refusal> {
    match measurement {
        Measurement::NotMeasured => assess(&Presented {
            entry: &gathered.entry,
            reproduction: gathered.reproduction,
            verdict: None,
            held_out: None,
        }),
        Measurement::Measured { measured, evidence } => {
            let verdict = judge(
                gathered.registration.as_ref(),
                gathered.entry.specification,
                measured,
                *evidence,
            );
            assess(&Presented {
                entry: &gathered.entry,
                reproduction: gathered.reproduction,
                verdict: Some(&verdict),
                // From the claim, like the horizon: a structure chosen after the result would judge it
                // out of sample against a partition picked afterwards.
                held_out: gathered.registration.as_ref().and_then(PreRegistration::held_out),
            })
        }
    }
}

use crate::promotion::assess;

/// Two kinds: a term that said no is the operator's next work. A stage sequence that does not join is a wrong run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotCleared {
    Terms(Refusal),
    Stage(StageError),
}

impl core::fmt::Display for NotCleared {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NotCleared::Terms(r) => write!(f, "{r}"),
            NotCleared::Stage(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for NotCleared {}

/// Clear the first stage. The terms are `assess`'s and are not softened. A strategy the ledger
/// does not record clears nothing after it.
pub fn clear_backtest(
    gathered: &Gathered,
    measurement: &Measurement<'_>,
) -> Result<Cleared<Backtest>, NotCleared> {
    let decision = decide(gathered, measurement).map_err(NotCleared::Terms)?;
    Cleared::first(decision).map_err(NotCleared::Stage)
}

/// Clear the paper stage. It is a different run (class and window differ) of the same strategy, and the join is checked.
pub fn clear_paper(
    prior: Cleared<Backtest>,
    gathered: &Gathered,
    measurement: &Measurement<'_>,
) -> Result<Cleared<Paper>, NotCleared> {
    let decision = decide(gathered, measurement).map_err(NotCleared::Terms)?;
    Cleared::<Paper>::after(prior, decision).map_err(NotCleared::Stage)
}
