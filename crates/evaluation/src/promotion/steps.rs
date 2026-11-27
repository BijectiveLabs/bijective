// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The path from a backtest to a live session as steps. One stage sequence, one place, so a report and a
//! command cannot drift into two answers about the same strategy.

use std::collections::BTreeMap;

use crate::evidence::{Evidence, EvidenceError, Outcome};
use crate::measure::measure;
use bijective_event_store::digest::Digest;
use bijective_strategy_engine::runs::{Research, RunSpec};

use crate::promotion::Refusal;
use crate::promotion::path::{
    Gathered, Measurement, NotCleared, clear_backtest, clear_paper, evidence_of, gather, runs_of,
};
use crate::promotion::stage::{Backtest, Cleared, StageError};

#[derive(Debug, Clone)]
pub struct Step {
    pub stage: &'static str,
    pub said: String,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Declared,
    /// A lookup, never an identity: the scaffold names a strategy after its file, and what was found is said.
    ByFileName,
    /// A program path: runs from before names were declared, and projects whose file names nothing.
    ByPath,
    /// Nothing has run under either reading. The declared one is reported as a new run would be recorded.
    Neither,
}

/// Declared name first, then the path. Trying only one would silently lose a history.
#[must_use]
pub fn resolve(root: &std::path::Path, typed: &str) -> (Digest, Known) {
    let declared = RunSpec::<Research>::digest_of_name(typed);
    if runs_of(root, declared).is_ok_and(|runs| !runs.is_empty()) {
        return (declared, Known::Declared);
    }

    if let Some(stem) = std::path::Path::new(typed).file_stem().and_then(|s| s.to_str()) {
        let by_stem = RunSpec::<Research>::digest_of_name(stem);
        if by_stem != declared && runs_of(root, by_stem).is_ok_and(|runs| !runs.is_empty()) {
            return (by_stem, Known::ByFileName);
        }
    }

    let by_path = strategy_of(typed);
    if runs_of(root, by_path).is_ok_and(|runs| !runs.is_empty()) {
        return (by_path, Known::ByPath);
    }

    (declared, Known::Neither)
}

/// The digest as strategies were known before names. It carries no class or window, so it names every run.
#[must_use]
pub fn strategy_of(program: &str) -> Digest {
    RunSpec::<Research>::declare(
        program.to_string(),
        Vec::new(),
        BTreeMap::new(),
        Some(0),
        bijective_strategy_engine::runs::Exchange::Whole,
    )
    .expect("a program is a specification")
    .strategy()
}

/// Each promotion stage the strategy has reached, what stopped it and the next step. Errors
/// reading the record are steps too, so a node that could not answer does not read as having
/// nothing to say.
pub fn walk(root: &std::path::Path, program: &str, confidence: i128) -> Vec<Step> {
    let (strategy, _) = resolve(root, program);
    let mut steps = Vec::new();
    let runs = match runs_of(root, strategy) {
        Ok(runs) => runs,
        Err(e) => {
            steps.push(Step { stage: "backtest", said: format!("{e}"), next: None });
            return steps;
        }
    };
    let backtest = backtest_step(root, program, confidence, &runs);
    let cleared = match backtest {
        Ok((cleared, step)) => {
            steps.push(step);
            cleared
        }
        Err(step) => {
            steps.push(step);
            return steps;
        }
    };
    steps.extend(paper_steps(root, program, confidence, &runs, cleared));
    steps
}

/// The newest bounded run, measured and judged against the backtest terms.
fn backtest_step(
    root: &std::path::Path,
    program: &str,
    confidence: i128,
    runs: &[(Digest, String)],
) -> Result<(Cleared<Backtest>, Step), Step> {
    let stopped = |said: String, next: Option<String>| Step { stage: "backtest", said, next };
    if runs.is_empty() {
        return Err(stopped(
            "nothing recorded".to_string(),
            Some(format!(
                "run it bounded: bijective-strategy-engine --store <store> --strategy {program} \
                 --market <market-stream> --to-position <position>"
            )),
        ));
    }
    let Some(back) = latest(runs, "research") else {
        return Err(stopped(
            format!("{} run(s) recorded, none of them bounded", runs.len()),
            Some("run it with --to-position, which declares the window it reads".to_string()),
        ));
    };
    let gathered = read(root, back).map_err(|said| stopped(said, None))?;
    match measure_and_clear(root, &gathered, confidence, clear_backtest) {
        Ok(cleared) => {
            let said = format!("cleared on {} held-out position(s)", cleared.held_out_positions());
            Ok((cleared, stopped(said, None)))
        }
        Err(not) => Err(stopped(not.said, Some(not.next))),
    }
}

/// The newest live session, judged against the paper terms on top of the cleared backtest. A
/// cleared paper stage is reported ready to be decided. Nothing here starts a strategy.
fn paper_steps(
    root: &std::path::Path,
    program: &str,
    confidence: i128,
    runs: &[(Digest, String)],
    cleared: Cleared<Backtest>,
) -> Vec<Step> {
    let stopped = |said: String, next: Option<String>| vec![Step { stage: "paper", said, next }];
    let Some(session) = latest(runs, "live") else {
        return stopped(
            "no session".to_string(),
            Some(format!(
                "run one against live data, reaching no venue: bijective-strategy-engine \
                 --store <store> --strategy {program} --market <market-stream> --stage paper"
            )),
        );
    };
    let gathered = match read(root, session) {
        Ok(g) => g,
        Err(said) => return stopped(said, None),
    };
    match measure_and_clear(root, &gathered, confidence, |g, m| clear_paper(cleared, g, m)) {
        Ok(paper) => vec![
            Step {
                stage: "paper",
                said: format!("cleared on {} held-out position(s)", paper.held_out_positions()),
                next: None,
            },
            Step {
                stage: "live",
                said: "ready to be decided. Nothing here starts a strategy.".to_string(),
                next: None,
            },
        ],
        Err(not) => stopped(not.said, Some(not.next)),
    }
}

fn latest(runs: &[(Digest, String)], class: &str) -> Option<Digest> {
    runs.iter().filter(|(_, c)| c == class).map(|(run, _)| *run).next_back()
}

fn read(root: &std::path::Path, run: Digest) -> Result<Gathered, String> {
    match gather(root, run) {
        Ok(Some(gathered)) => Ok(gathered),
        Ok(None) => {
            Err(format!("no entry for {}, so this node cannot say it ran", &run.to_hex()[..16]))
        }
        Err(e) => Err(format!("{e}")),
    }
}

fn measure_and_clear<T>(
    root: &std::path::Path,
    gathered: &Gathered,
    confidence: i128,
    clear: impl FnOnce(&Gathered, &Measurement<'_>) -> Result<T, NotCleared>,
) -> Result<T, Stopped> {
    let measured = measure(root, gathered.run).ok();
    let measurement = match &measured {
        Some(m) if m.observations > 0 => Measurement::Measured {
            measured: &m.measured,
            evidence: evidence_of(gathered, m, confidence),
        },
        _ => Measurement::NotMeasured,
    };

    clear(gathered, &measurement)
        .map_err(|not| Stopped { said: not.to_string(), next: next_action(&not, gathered) })
}

struct Stopped {
    said: String,
    next: String,
}

/// One arm per term: a catch-all would give advice about a different term for one not yet written.
fn next_action(not: &NotCleared, gathered: &Gathered) -> String {
    match not {
        NotCleared::Terms(Refusal::NotReproduced) => {
            "run the same specification again; one run cannot be stood behind".to_string()
        }
        NotCleared::Terms(Refusal::DidNotReproduce { .. }) => {
            "the two runs emitted different bytes: the strategy is not deterministic over its window"
                .to_string()
        }
        NotCleared::Terms(Refusal::OutputUnusable { exit }) => {
            format!("the run ended {exit}; run it to completion")
        }
        NotCleared::Terms(Refusal::NotMeasured) => {
            "nothing measured it: no proposal it made crossed the touch, so there is nothing to judge"
                .to_string()
        }
        NotCleared::Terms(Refusal::ClaimNotMet { outcome }) => match outcome {
            Outcome::Unregistered => format!(
                "declare a claim for this run before it runs again: declare_claim <store> <program> \
                 {} <hypothesis> <measure> at-least <threshold> <horizon> <folds>",
                if gathered.entry.class == "live" { "live" } else { "<to-position>" }
            ),
            Outcome::Unmeasured { measure } => format!(
                "the claim is about {measure} and nothing produced that figure; the measures are \
                 spread_bps, cost_floor_bps, gross_return_bps and net_after_spread_bps"
            ),
            Outcome::NotMet { observed } => {
                format!("the figure was {observed}: the strategy did not do what was claimed")
            }
            Outcome::WrongRun { .. } => {
                "the claim names another specification; declare one for this run".to_string()
            }
            Outcome::Met { .. } => "nothing: the claim was met".to_string(),
        },
        NotCleared::Terms(Refusal::InSampleOnly) => {
            "declare the folds this result is judged on, with the claim".to_string()
        }
        NotCleared::Terms(Refusal::EvidenceShort { evidence }) => match evidence {
            Evidence::Insufficient { short_by, .. } => {
                format!("{short_by} more observation(s): run it over more history")
            }
            Evidence::Unknowable { cause: EvidenceError::NotPositive } => {
                "the measured figures have no spread between them, so the evidence cannot be sized. \
                 Every proposal a bounded run makes is priced at its window's end - the platform \
                 knows what it sent and not which event the strategy decided on - so a bounded run \
                 of one admission cannot carry a claim's evidence"
                    .to_string()
            }
            Evidence::Unknowable { cause: EvidenceError::EffectIsZero } => {
                "the claim's threshold is zero, and an effect of zero is not distinguishable at any \
                 sample size"
                    .to_string()
            }
            Evidence::Unknowable { cause: EvidenceError::Overflow } => {
                "the requirement does not fit: the claim's dispersion and effect are too far apart"
                    .to_string()
            }
            Evidence::Sufficient { .. } => "nothing: the evidence covers the claim".to_string(),
        },
        NotCleared::Stage(StageError::PriorStageWasAnotherStrategy { .. }) => {
            "this session is of a different program than the backtest that was cleared".to_string()
        }
        NotCleared::Stage(StageError::StrategyNotRecorded { .. }) => {
            "this run predates the ledger recording strategies; run it again".to_string()
        }
        NotCleared::Stage(StageError::PriorStageRanOtherBytes { prior, this }) => format!(
            "the cleared stage ran version {} and this one ran {}; run the earlier stage again on \
             this version",
            &prior.to_hex()[..12],
            &this.to_hex()[..12]
        ),
        NotCleared::Stage(StageError::BytesNotRecorded { .. }) => {
            "this run predates the ledger recording which version ran; run it again".to_string()
        }
    }
}
