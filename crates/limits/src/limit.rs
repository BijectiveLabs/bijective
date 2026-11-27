// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Limit declaration and evaluation against an exposure snapshot.

use super::*;

impl core::fmt::Display for LimitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LimitError::UnitMismatch { kind, expected, declared } => {
                write!(f, "{kind:?} measures {expected:?} and was declared in {declared:?}")
            }
            LimitError::Unreachable { kind } => {
                write!(f, "{kind:?} can never be reached by any observation")
            }
        }
    }
}

/// Time arrives as data: no clock is read here, so decisions are reproducible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureSnapshot {
    pub observed_at_ns: i128,
    /// Scoped: offsetting venues leave the net near zero while one sits at its maximum.
    /// A scope and kind absent here is unjudged, not zero.
    pub figures: Vec<(Scope, LimitKind, Threshold)>,
}

/// Carried on every result so a halt can be justified by whether the figure was live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale {
        age_ns: i128,
    },
    /// Stamped after the instant asked about. Not folded into staleness: a negative age reads as
    /// within bound, so a producer with a fast clock would suppress every staleness finding.
    Implausible {
        skew_ns: i128,
    },
}

/// Not a single verdict: a breach and an unjudged limit can coexist and need different responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub breaches: Vec<ScopedBreach>,
    pub unjudged: Vec<LimitKind>,
    pub freshness: Freshness,
}

impl Evaluation {
    /// Absence of a breach is not safety: an unjudged limit or stale figure means the platform does not know.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.breaches.is_empty()
            && self.unjudged.is_empty()
            && matches!(self.freshness, Freshness::Fresh)
    }
}

/// `freshness_bound_ns` is operator-declared, not a constant of this crate.
/// Every limit is judged and every finding returned. It never stops at the first.
#[must_use]
pub fn evaluate(
    limits: &[Limit],
    snapshot: &ExposureSnapshot,
    asked_at_ns: i128,
    freshness_bound_ns: i128,
) -> Evaluation {
    let mut breaches = Vec::new();
    let mut unjudged = Vec::new();

    for limit in limits {
        // Scope and kind both: kind alone matched the nearest figure of the right unit.
        match snapshot
            .figures
            .iter()
            .find(|(scope, kind, _)| *scope == limit.scope && *kind == limit.kind)
        {
            // A wrong-unit figure is reported unjudged: a meaningless "no breach" is the incident itself.
            Some((_, _, observed)) => match limit.judge(*observed) {
                Ok(Some(breach)) => {
                    breaches.push(ScopedBreach { scope: limit.scope.clone(), breach })
                }
                Ok(None) => {}
                Err(LimitError::UnitMismatch { kind, .. } | LimitError::Unreachable { kind }) => {
                    unjudged.push(kind);
                }
            },
            None => unjudged.push(limit.kind),
        }
    }

    Evaluation {
        breaches,
        unjudged,
        freshness: freshness_of(snapshot.observed_at_ns, asked_at_ns, freshness_bound_ns),
    }
}

pub(crate) fn freshness_of(observed_at_ns: i128, asked_at_ns: i128, bound_ns: i128) -> Freshness {
    let age_ns = asked_at_ns - observed_at_ns;
    if age_ns < 0 {
        // Stamped after the instant asked about: the clocks disagree. See `Implausible`.
        Freshness::Implausible { skew_ns: -age_ns }
    } else if age_ns > bound_ns {
        Freshness::Stale { age_ns }
    } else {
        // At the bound is still fresh: a declared maximum is permitted.
        Freshness::Fresh
    }
}

/// Observed value and threshold travel with the finding so the record cannot disagree with itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Breach {
    /// Carried because safe-state selection turns on it.
    pub level: ScopeLevel,
    pub kind: LimitKind,
    pub observed: Threshold,
    /// Both figures carry their unit so a leverage reads as a proportion, not a bare number.
    pub maximum: Threshold,
}

/// Beside `Breach`, not inside: `Breach` travels in the `Copy` `PreTradeVerdict` and a
/// `Scope` carries a `String`. Without it a venue breach could not say which venue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedBreach {
    pub scope: Scope,
    pub breach: Breach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limit {
    /// What the limit is about, not just its width: level alone matched a venue limit to the whole book.
    pub(crate) scope: Scope,
    pub(crate) kind: LimitKind,
    pub(crate) maximum: Threshold,
}

impl Limit {
    /// The only constructor. Checked at run time because limits come from operator configuration.
    pub fn declare(scope: Scope, kind: LimitKind, maximum: Threshold) -> Result<Self, LimitError> {
        if maximum.unit() != kind.unit() {
            return Err(LimitError::UnitMismatch {
                kind,
                expected: kind.unit(),
                declared: maximum.unit(),
            });
        }
        // At the representable ceiling nothing can exceed it, so the limit would never fire.
        if maximum.scaled().checked_add(1).is_none() {
            return Err(LimitError::Unreachable { kind });
        }
        Ok(Self { scope, kind, maximum })
    }

    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Refuses a cross-unit comparison rather than returning no breach. Exactly at the maximum is compliant.
    pub fn judge(&self, observed: Threshold) -> Result<Option<Breach>, LimitError> {
        if observed.unit() != self.kind.unit() {
            return Err(LimitError::UnitMismatch {
                kind: self.kind,
                expected: self.kind.unit(),
                declared: observed.unit(),
            });
        }
        if self.kind.size_of(observed) > self.maximum.scaled() {
            return Ok(Some(Breach {
                level: self.scope.level(),
                kind: self.kind,
                observed,
                maximum: self.maximum,
            }));
        }
        Ok(None)
    }

    /// Proves reachability constructively: a limit that can never fire shows on the register as protection.
    #[must_use]
    pub fn witness_breach(&self) -> Option<Threshold> {
        let beyond = self.maximum.scaled().checked_add(1)?;
        Some(match self.kind.unit() {
            Unit::Money => Threshold::Money(Amount::from_scaled(beyond)),
            Unit::Ratio => Threshold::Ratio(Ratio::from_scaled(beyond)),
        })
    }

    #[must_use]
    pub fn level(&self) -> ScopeLevel {
        self.scope.level()
    }

    #[must_use]
    pub const fn kind(&self) -> LimitKind {
        self.kind
    }
}

/// Each variant is a control that would sit on the register looking like protection while protecting nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    UnitMismatch { kind: LimitKind, expected: Unit, declared: Unit },
    Unreachable { kind: LimitKind },
}
