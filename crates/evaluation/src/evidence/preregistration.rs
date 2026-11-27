// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What was claimed, declared before the answer existed.
//! Research integrity. A registration names the run digest, so it binds before the answer exists.

use crate::evidence::walk_forward::{Fold, WalkForward, WalkForwardError};
use bijective_event_store::digest::Digest;
use bijective_strategy_engine::runs::{Authority, RunSpec};

use crate::evidence::Evidence;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    AtLeast,
    AtMost,
}

/// A rule the platform can apply instead of prose a person re-reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRule {
    pub measure: String,
    pub direction: Direction,
    /// The threshold, in the same fixed scale and unit as the measure it judges.
    pub threshold: i128,
}

impl DecisionRule {
    #[must_use]
    pub fn met_by(&self, observed: i128) -> bool {
        match self.direction {
            Direction::AtLeast => observed >= self.threshold,
            Direction::AtMost => observed <= self.threshold,
        }
    }
}

/// Why a registration could not be declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    NoHypothesis,
    /// A rule naming no measure would be met by any figure.
    NoMeasure,
    /// Reached only by old records. An operator may not declare it.
    NoHorizon,
    /// Zero positions marks at the decision book, so the return is zero by arithmetic.
    HorizonOfNothing,
    /// The held-out structure does not hold together, and the rule that stopped it.
    Structure(WalkForwardError),
}

/// The window is inside the run identity, so a registration cannot move to other history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreRegistration {
    hypothesis: String,
    rule: DecisionRule,
    run: Digest,
    horizon: Horizon,
    held_out: Option<WalkForward>,
}

impl PreRegistration {
    /// Takes the spec, not a digest, so a caller cannot register a run it has not described.
    /// Folds are checked against the run's window. Empty folds mean nothing held out.
    pub fn declare<A: Authority>(
        hypothesis: impl Into<String>,
        rule: DecisionRule,
        horizon: Horizon,
        folds: Vec<Fold>,
        spec: &RunSpec<A>,
    ) -> Result<Self, RegistrationError> {
        let hypothesis = hypothesis.into();
        if hypothesis.trim().is_empty() {
            return Err(RegistrationError::NoHypothesis);
        }
        if rule.measure.trim().is_empty() {
            return Err(RegistrationError::NoMeasure);
        }

        // NotDeclared is what pre-horizon records read as. Declaring it now would be deliberate.
        if matches!(horizon, Horizon::NotDeclared) {
            return Err(RegistrationError::NoHorizon);
        }
        if matches!(horizon, Horizon::After { positions: 0 }) {
            return Err(RegistrationError::HorizonOfNothing);
        }

        let held_out = if folds.is_empty() {
            None
        } else {
            Some(WalkForward::declare(folds, spec.window()).map_err(RegistrationError::Structure)?)
        };

        Ok(Self { hypothesis, rule, run: spec.identity(), horizon, held_out })
    }

    #[must_use]
    pub fn hypothesis(&self) -> &str {
        &self.hypothesis
    }

    #[must_use]
    pub fn rule(&self) -> &DecisionRule {
        &self.rule
    }

    #[must_use]
    pub fn run(&self) -> Digest {
        self.run
    }

    #[must_use]
    pub fn horizon(&self) -> Horizon {
        self.horizon
    }

    /// `None` means no out-of-sample test was declared. An empty structure would be a test over nothing.
    #[must_use]
    pub fn held_out(&self) -> Option<&WalkForward> {
        self.held_out.as_ref()
    }
}

impl PreRegistration {
    /// Not public: only the registry rebuilds a claim from the stream.
    pub(crate) fn as_read(
        run: Digest,
        hypothesis: String,
        rule: DecisionRule,
        horizon: Horizon,
        held_out: Option<WalkForward>,
    ) -> Self {
        Self { hypothesis, rule, run, horizon, held_out }
    }
}

/// Positions held before the mark. Part of the claim: a measurer picking its own exit would
/// answer an unregistered question. Counted in positions, not time, so it reproduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Horizon {
    After {
        positions: u64,
    },
    NotHeld,
    /// Pre-horizon record. Distinct from `NotHeld`, which an operator states.
    NotDeclared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    pub measure: String,
    pub value: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Met {
        observed: i128,
    },
    NotMet {
        observed: i128,
    },
    /// No figure for the registered measure. The experiment did not answer, which is not a rejection.
    Unmeasured {
        measure: String,
    },
    /// The registration is for a different run identity.
    WrongRun {
        registered: Digest,
        ran: Digest,
    },
    /// No registration. Never inferred as either answer.
    Unregistered,
}

impl Outcome {
    /// Only a met rule carries a promotion.
    #[must_use]
    pub fn supports_promotion(&self) -> bool {
        matches!(self, Outcome::Met { .. })
    }
}

/// Carried, not dropped: hiding unregistered figures conceals what was looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unregistered {
    pub measure: String,
    pub value: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub outcome: Outcome,
    pub unregistered: Vec<Unregistered>,
    pub evidence: Evidence,
}

/// `ran` comes from the run ledger, not the registration. Comparing a registration to itself proves nothing.
#[must_use]
pub fn judge(
    registration: Option<&PreRegistration>,
    ran: Digest,
    measured: &[Measured],
    evidence: Evidence,
) -> Verdict {
    let Some(registration) = registration else {
        return Verdict {
            outcome: Outcome::Unregistered,
            unregistered: measured
                .iter()
                .map(|m| Unregistered { measure: m.measure.clone(), value: m.value })
                .collect(),
            evidence,
        };
    };

    if registration.run != ran {
        return Verdict {
            outcome: Outcome::WrongRun { registered: registration.run, ran },
            unregistered: Vec::new(),
            evidence,
        };
    }

    let declared = registration.rule.measure.as_str();
    let outcome = match measured.iter().find(|m| m.measure == declared) {
        Some(m) if registration.rule.met_by(m.value) => Outcome::Met { observed: m.value },
        Some(m) => Outcome::NotMet { observed: m.value },
        None => Outcome::Unmeasured { measure: declared.to_string() },
    };

    Verdict {
        outcome,
        unregistered: measured
            .iter()
            .filter(|m| m.measure != declared)
            .map(|m| Unregistered { measure: m.measure.clone(), value: m.value })
            .collect(),
        evidence,
    }
}
