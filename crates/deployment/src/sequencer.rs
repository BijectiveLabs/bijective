// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One answer for the whole node (the deployment kit: six units must feel like one install).
//! It refuses to compute a comforting answer.

use std::path::Path;

use bijective_common::readiness::{Health, ReadinessError, read_at};

use crate::manifest::{Deployment, ProgramClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramState {
    pub name: String,
    pub class: ProgramClass,
    pub health: Option<Health>,
    /// A claim exists and cannot be accepted: stale, unreadable, or from an unknown build.
    /// Distinct from no claim: never reported points at a start problem, stopped reporting at a death.
    pub lapsed: bool,
    /// The program's own words, or the reason we have none.
    pub detail: String,
}

impl ProgramState {
    pub fn is_ready(&self) -> bool {
        self.health.is_some_and(Health::is_ready)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.health, Some(Health::Failed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    /// No core program is reporting at all.
    Down,
    /// A core program is reporting and not yet ready.
    Starting,
    /// Every core program is ready. The node may be operated. It is not trading: the trading state rests at halted.
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub state: NodeState,
    /// Why, naming the program responsible.
    pub because: String,
    pub programs: Vec<ProgramState>,
}

impl Node {
    /// Operational programs failing or lapsed while the node is otherwise up.
    /// Kept apart from node state: the core-runs-alone rule says the core does not need them, but it must not be silent.
    pub fn degradations(&self) -> Vec<&ProgramState> {
        self.programs
            .iter()
            .filter(|p| p.class == ProgramClass::Operational && (p.is_failed() || p.lapsed))
            .collect()
    }
}

/// Read every program's claim and reduce it to one.
/// `now_ns` is passed in so staleness can be tested against a program that stopped reporting.
pub fn assess_node(deployment: &Deployment, run_dir: &Path, now_ns: i128) -> Node {
    let mut programs = Vec::new();

    for program in &deployment.programs {
        let path = run_dir.join(format!("{}.json", program.name));
        let (health, lapsed, detail) = match read_at(&path, now_ns) {
            Ok(doc) => (Some(doc.health), false, doc.detail),
            Err(ReadinessError::Absent) => (None, false, "has not reported".to_string()),
            Err(e) => (None, true, e.to_string()),
        };
        programs.push(ProgramState {
            name: program.name.clone(),
            class: program.class,
            health,
            lapsed,
            detail,
        });
    }

    let core: Vec<&ProgramState> =
        programs.iter().filter(|p| p.class == ProgramClass::Core).collect();

    // Failure dominates absence: reporting the softer answer is how a fault waits to be noticed.
    if let Some(failed) = core.iter().find(|p| p.is_failed()) {
        let because = format!("{} failed: {}", failed.name, failed.detail);
        return Node { state: NodeState::Failed, because, programs };
    }

    // A stale claim is a program that ran and stopped. Fail closed and carry the reason.
    if let Some(gone) = core.iter().find(|p| p.lapsed) {
        let because = format!("{} stopped reporting: {}", gone.name, gone.detail);
        return Node { state: NodeState::Failed, because, programs };
    }

    if core.iter().all(|p| p.health.is_none()) {
        let because = "no core program has reported; the node has not been started".to_string();
        return Node { state: NodeState::Down, because, programs };
    }

    if let Some(waiting) = core.iter().find(|p| !p.is_ready()) {
        let because = format!("{} is not ready: {}", waiting.name, waiting.detail);
        return Node { state: NodeState::Starting, because, programs };
    }

    let because = format!("{} core program(s) ready; trading is a separate state", core.len());
    Node { state: NodeState::Ready, because, programs }
}
