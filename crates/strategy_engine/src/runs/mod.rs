// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What an agent run is, who may make it, and what it may emit.
//! `docs/concepts/strategies.md` carries why the classes are types rather than a field.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use bijective_event_store::digest::{Digest, sha256};
// The platform's own object: two definitions of a strategy's output would be two vocabularies for one thing.
pub use bijective_model::Proposal;

pub mod ledger;

pub use ledger::{ExitReason, RunLedgerEntry};

/// Never a wall-clock time: a date depends on what the store held at start, a sequence is the same run forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    ToPosition(u64),
    ToPresent,
    /// The platform's own record. No market data at all.
    RecordOnly,
}

/// How the supervisor fed a run its window.
/// Inside the run's identity: a strategy sent its whole window may answer the first event holding the last. Different experiments over one history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exchange {
    /// The window is sent whole, then the output is read: fast, and it cannot place a decision.
    Whole,
    /// One event, then the answer, then the next: the supervisor assigns the position instead of taking a claim, at a turn per event.
    Stepwise,
}

impl Exchange {
    /// Whether a decision position from a run fed this way is a fact or a claim.
    #[must_use]
    pub fn places_decisions(self) -> bool {
        matches!(self, Exchange::Stepwise)
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Exchange::Whole => "whole",
            Exchange::Stepwise => "stepwise",
        }
    }

    /// Refused, not defaulted: the two produce different runs, and one cannot carry a claim that rests on when a decision was made.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "whole" => Some(Exchange::Whole),
            "stepwise" => Some(Exchange::Stepwise),
            _ => None,
        }
    }
}

/// `None` is a class this build does not know: ask the stricter question, since a class nothing here can name has not been shown unreplayable.
#[must_use]
pub fn replays(class: &str) -> Option<bool> {
    match class {
        Research::NAME => Some(Research::REPLAYS),
        Live::NAME => Some(Live::REPLAYS),
        Observer::NAME => Some(Observer::REPLAYS),
        Auditor::NAME => Some(Auditor::REPLAYS),
        _ => None,
    }
}

pub trait Authority {
    /// What a run of this class emits. There is no second kind.
    type Artefact;

    const NAME: &'static str;

    /// Whether a run of this class can be replayed to the same bytes.
    /// Research integrity is claimed over a declared window. A class that reads the present makes no such claim.
    const REPLAYS: bool;

    /// Fixed by the class, not chosen beside it.
    fn window(bound: Option<u64>) -> Window;
}

/// Reads the store to a declared position. Emits proposals the Control Engine then judges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Research;

/// The proposal stream a bounded research run writes to.
/// Not the stream the Control Engine reads: it never asks where an entry came from, so a backtest on a trading node would put research in front of order placement.
/// A run's ledger entry records the stream it wrote to, so readers follow the entry.
pub const RESEARCH_PROPOSALS: &str = "proposals-research";

/// Which stage of the path towards capital a live session runs under.
/// Fixes the stream a session writes to and nothing else: a paper session's proposals are not on the stream the Control Engine judges, not merely flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Paper,
    Live,
}

impl Stage {
    /// Separate streams, not a flag an engine must check: an engine reading one cannot act on the other's contents by any path.
    #[must_use]
    pub fn proposals(self) -> &'static str {
        match self {
            Stage::Paper => bijective_serialization::signal::node::PAPER.proposals,
            Stage::Live => bijective_serialization::signal::node::LIVE.proposals,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Stage::Paper => "paper",
            Stage::Live => "live",
        }
    }

    /// Refused, not defaulted: an unset stage would route a session's proposals to a venue no configuration selected.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "paper" => Some(Stage::Paper),
            "live" => Some(Stage::Live),
            _ => None,
        }
    }
}

/// Reads the market as it is recorded and emits proposals. The live half of the strategy lifetime model.
/// The fourth combination: research is bounded and an observer emits nothing the order path acts on, so one strategy in two lifetimes is a change of class, not of program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live;

/// Reads the store to the present. Emits observations about how the platform is behaving.
/// Nothing it emits can reach the order path: an `Observation` is not an `OrderIntent` and there is no conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observer;

/// Reads the platform's own record. Emits findings about that record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Auditor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub subject: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub stream: String,
    pub position: u64,
    pub detail: String,
}

impl Authority for Research {
    type Artefact = Proposal;
    const NAME: &'static str = "research";
    const REPLAYS: bool = true;

    fn window(bound: Option<u64>) -> Window {
        // No bound is not the present for this class: a research run with nothing declared reads nothing, instead of quietly widening its window.
        bound.map_or(Window::ToPosition(0), Window::ToPosition)
    }
}

impl Authority for Live {
    type Artefact = Proposal;
    const NAME: &'static str = "live";
    const REPLAYS: bool = false;

    fn window(_bound: Option<u64>) -> Window {
        // A bound would be a scheduled outage: a strategy stopped at an undeclared position would look like one that chose to stop trading.
        Window::ToPresent
    }
}

impl Authority for Observer {
    type Artefact = Observation;
    const NAME: &'static str = "observer";
    const REPLAYS: bool = false;

    fn window(_bound: Option<u64>) -> Window {
        Window::ToPresent
    }
}

impl Authority for Auditor {
    type Artefact = AuditFinding;
    const NAME: &'static str = "auditor";
    const REPLAYS: bool = true;

    fn window(_bound: Option<u64>) -> Window {
        Window::RecordOnly
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    NoWindow,
    NoProgram,
}

/// A run, declared before it starts and unchanged afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec<A: Authority> {
    /// What an operator calls this strategy. Identity for the promotion stages only: a name survives the file moving, and recorded content notices the file changing.
    name: Option<String>,
    program: String,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    window: Window,
    exchange: Exchange,
    /// Every market a run reads with the position it ends at (none for a session, which reads to
    /// the present), when it reads more than one. Empty for one market, so those identities are
    /// what they were before runs could read several.
    markets: Vec<(String, Option<u64>)>,
    class: PhantomData<A>,
}

impl<A: Authority> RunSpec<A> {
    /// The window comes from the class. The caller supplies only its bound, so a research run cannot be handed the present.
    pub fn declare(
        program: impl Into<String>,
        arguments: Vec<String>,
        environment: BTreeMap<String, String>,
        bound: Option<u64>,
        exchange: Exchange,
    ) -> Result<Self, SpecError> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err(SpecError::NoProgram);
        }
        if matches!(A::window(bound), Window::ToPosition(_)) && bound.is_none() {
            return Err(SpecError::NoWindow);
        }

        Ok(Self {
            name: None,
            program,
            arguments,
            environment,
            window: A::window(bound),
            exchange,
            markets: Vec::new(),
            class: PhantomData,
        })
    }

    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    #[must_use]
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }

    #[must_use]
    pub fn window(&self) -> Window {
        self.window
    }

    #[must_use]
    pub fn exchange(&self) -> Exchange {
        self.exchange
    }

    #[must_use]
    pub fn class(&self) -> &'static str {
        A::NAME
    }

    /// A run that reads several markets: each stream and the position it ends at are part of
    /// what the run is, so the same program over other markets is another experiment.
    #[must_use]
    pub fn reading(mut self, markets: Vec<(String, Option<u64>)>) -> Self {
        self.markets = markets;
        self
    }

    /// Given after `declare` so callers that only name a program need not pass a `None` that is never read.
    #[must_use]
    pub fn called(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !name.trim().is_empty() {
            self.name = Some(name);
        }
        self
    }

    /// The name an operator gave the strategy, which its proposals carry so a limit can bind it.
    #[must_use]
    pub fn strategy_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The strategy this run is of, not its identity: the promotion stages follows the program and what it was asked to do.
    /// The environment is deliberately absent: library location differs between a build tree and a node, and a Python strategy was invisible to the promotion stages.
    #[must_use]
    pub fn strategy(&self) -> Digest {
        // Digested in a space a program can never reach: the tag ends in a NUL and no path holds one, so a name never collides with a path and an unnamed strategy digests as before.
        if let Some(name) = &self.name {
            let mut bytes = b"declared\0".to_vec();
            bytes.extend_from_slice(name.as_bytes());
            return sha256(&bytes);
        }

        let mut bytes = Vec::new();
        bytes.extend_from_slice(self.program.as_bytes());
        bytes.push(0);
        for argument in &self.arguments {
            bytes.extend_from_slice(argument.as_bytes());
            bytes.push(0);
        }
        sha256(&bytes)
    }

    #[must_use]
    pub fn digest_of_name(name: &str) -> Digest {
        let mut bytes = b"declared\0".to_vec();
        bytes.extend_from_slice(name.as_bytes());
        sha256(&bytes)
    }

    /// Same specification, same experiment. A changed one is a different run, not a rerun.
    #[must_use]
    pub fn identity(&self) -> Digest {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(A::NAME.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(self.program.as_bytes());
        bytes.push(0);
        for argument in &self.arguments {
            bytes.extend_from_slice(argument.as_bytes());
            bytes.push(0);
        }
        bytes.push(1);
        // Ordered: a map's arbitrary order would give one run two identities.
        for (key, value) in &self.environment {
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(b'=');
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        bytes.push(2);
        bytes.extend_from_slice(self.exchange.name().as_bytes());
        bytes.push(0);
        match self.window {
            Window::ToPosition(p) => {
                bytes.push(0);
                bytes.extend_from_slice(&p.to_be_bytes());
            }
            Window::ToPresent => bytes.push(1),
            Window::RecordOnly => bytes.push(2),
        }
        if !self.markets.is_empty() {
            bytes.push(3);
            for (stream, through) in &self.markets {
                bytes.extend_from_slice(stream.as_bytes());
                bytes.push(0);
                match through {
                    Some(through) => {
                        bytes.push(0);
                        bytes.extend_from_slice(&through.to_be_bytes());
                    }
                    None => bytes.push(1),
                }
            }
        }
        sha256(&bytes)
    }
}

/// Takes what a run emitted. Parameterised by class, so a collector for an observer has no method that accepts a proposal.
#[derive(Debug, Default)]
pub struct Collector<A: Authority> {
    taken: Vec<A::Artefact>,
    class: PhantomData<A>,
}

impl<A: Authority> Collector<A> {
    #[must_use]
    pub fn new() -> Self {
        Self { taken: Vec::new(), class: PhantomData }
    }

    pub fn take(&mut self, artefact: A::Artefact) {
        self.taken.push(artefact);
    }

    #[must_use]
    pub fn taken(&self) -> &[A::Artefact] {
        &self.taken
    }
}

#[cfg(test)]
#[path = "../../tests/unit/runs/tests.rs"]
mod tests;
