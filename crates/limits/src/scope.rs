// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Scope levels and the entity a limit applies to.

use super::*;

impl core::fmt::Display for Entity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "desk {}, venue {}, strategy {}, instrument {}",
            self.desk, self.venue, self.strategy, self.instrument
        )
    }
}

/// Identified by hierarchy position so an operator sees the thing they configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub desk: String,
    /// The event model: a degrading venue is constrained by its own scope, so the level must exist.
    pub venue: String,
    pub strategy: String,
    pub instrument: String,
}

/// Levels are cumulative: an entity is bound by every level that covers it, not its nearest declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// The whole book. The one declaration that cannot leave an entity uncovered.
    Portfolio,
    Desk(String),
    Venue(String),
    Strategy(String),
    /// One asset across every venue, named by the base currency instruments are counted in, in
    /// lower case. `docs/decisions/0001-underlying-scope-level.md`.
    Underlying(String),
    Instrument(String),
}

/// Without the name: safe-state selection depends on how wide a breach is, not what it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeLevel {
    Portfolio,
    Desk,
    Venue,
    Strategy,
    Underlying,
    Instrument,
}

impl ScopeLevel {
    /// Widest first, per the event model. A further level is a decision record first
    /// (`docs/decisions/`).
    pub const ALL: [ScopeLevel; 6] = [
        ScopeLevel::Portfolio,
        ScopeLevel::Desk,
        ScopeLevel::Venue,
        ScopeLevel::Strategy,
        ScopeLevel::Underlying,
        ScopeLevel::Instrument,
    ];
}

impl Scope {
    #[must_use]
    pub fn level(&self) -> ScopeLevel {
        match self {
            Self::Portfolio => ScopeLevel::Portfolio,
            Self::Desk(_) => ScopeLevel::Desk,
            Self::Venue(_) => ScopeLevel::Venue,
            Self::Strategy(_) => ScopeLevel::Strategy,
            Self::Underlying(_) => ScopeLevel::Underlying,
            Self::Instrument(_) => ScopeLevel::Instrument,
        }
    }
}

/// Fixed, not open: a limit that cannot be computed shows on the register as protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LimitKind {
    GrossExposure,
    NetExposure,
    Concentration,
    Leverage,
    DailyLoss,
    /// How far today's result has fallen from its highest point today, in money.
    Drawdown,
    /// The venues' maintenance margin over the equity backing it: at one, a venue may liquidate.
    MarginUse,
}

impl LimitKind {
    /// Stated once so no two call sites can disagree about the unit.
    #[must_use]
    pub const fn unit(self) -> Unit {
        match self {
            Self::GrossExposure
            | Self::NetExposure
            | Self::Concentration
            | Self::DailyLoss
            | Self::Drawdown => Unit::Money,
            Self::Leverage | Self::MarginUse => Unit::Ratio,
        }
    }

    /// The size of a figure a limit of this kind is judged on. Net exposure is judged by its
    /// magnitude: a short is as far from flat as a long of the same size, and a signed comparison
    /// would let any short through. Every other kind is judged as signed, so a gain is no loss.
    #[must_use]
    pub fn size_of(self, figure: Threshold) -> i128 {
        match self {
            Self::NetExposure => figure.scaled().saturating_abs(),
            _ => figure.scaled(),
        }
    }
}
