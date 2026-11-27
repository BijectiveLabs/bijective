// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Safe-state selection, driving the trading state and verifying its effect.

use super::*;

impl core::fmt::Display for DriveFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DriveFailure::NotApplied { requested, observed } => {
                write!(f, "the substrate was asked for {requested:?} and reports {observed:?}")
            }
            DriveFailure::ExposureIncreasedAfterSafeState { kind, before, after } => {
                write!(f, "{kind:?} rose from {before} to {after} after the safe state was entered")
            }
            DriveFailure::PostconditionUnverifiable { kind } => {
                write!(f, "{kind:?} could not be checked after the safe state was entered")
            }
        }
    }
}

/// Driven through the substrate's own setter so the action covers every trading command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeState {
    Active,
    /// Rejects commands that extend an existing net position. A flat instrument is not constrained,
    /// so this does not stop new exposure.
    Reducing,
    Halted,
}

/// Needed because `Reducing` constrains nothing where nothing is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionHeld {
    Yes,
    No,
}

/// Taken from the venue assurance profile: it decides whether a safe state means what its name says on that venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReduceOnlySemantics {
    PlacementPrecondition,
    /// Clamps at fill time: a reduce-only order against no position still fills.
    FillTimeClamp,
}

/// Only an instrument breach on a held position at a placement-precondition venue gets `Reducing`.
/// Everything else halts (`docs/concepts/risk_and_limits.md`).
#[must_use]
pub const fn select_safe_state(
    breach: &Breach,
    position_held: PositionHeld,
    semantics: ReduceOnlySemantics,
) -> SafeState {
    match (breach.level, position_held, semantics) {
        (ScopeLevel::Instrument, PositionHeld::Yes, ReduceOnlySemantics::PlacementPrecondition) => {
            SafeState::Reducing
        }
        _ => SafeState::Halted,
    }
}

/// Shared by both modules. Every variant is a reason an operator can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeStateReason {
    /// Closes the startup window: a pre-trade check holding nothing must not return carry on.
    NeverPrimed,
    InputsStale {
        stale_for_ns: i128,
    },
    /// Not satisfied and not guessable: a limit fired on an invented figure cannot be explained.
    Unjudgeable {
        kind: LimitKind,
    },
    Breached(Breach),
    /// A reconciliation pass found the venue holding what the record does not.
    ReconciliationDiverged,
    /// A venue's evidence could not be read: nothing is known of what it holds.
    EvidenceUnread,
}

/// A port: the one trading state lives in the Control Engine, so linking it
/// here would make a cycle. It also keeps this crate dependency-free and the driver testable.
pub trait TradingStateControl {
    /// A request, not a guarantee. `current` reads back whether it took effect.
    fn set(&mut self, state: SafeState);

    fn current(&self) -> SafeState;
}

/// Each variant is a top-severity alarm: the platform recorded a stop that did not take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveFailure {
    NotApplied {
        requested: SafeState,
        observed: SafeState,
    },
    /// Honoured in name, not in effect, as a fill-time reduce-only clamp produces.
    ExposureIncreasedAfterSafeState {
        kind: LimitKind,
        before: Threshold,
        after: Threshold,
    },
    /// Not a pass: a missing figure must not silently disable the one check on an unhonoured halt.
    PostconditionUnverifiable {
        kind: LimitKind,
    },
}

/// The read-back is the function: a returned call only proves a call returned.
pub fn drive_safe_state(
    control: &mut impl TradingStateControl,
    target: SafeState,
) -> Result<(), DriveFailure> {
    control.set(target);

    let observed = control.current();
    if observed == target {
        Ok(())
    } else {
        Err(DriveFailure::NotApplied { requested: target, observed })
    }
}

/// Second half of the postcondition: a fill-time clamping venue passes the drive check and fails this one.
/// The before-side is a figure and the compared figure is returned.
pub fn verify_exposure_did_not_increase(
    before: Threshold,
    after: &ExposureSnapshot,
    scope: &Scope,
    kind: LimitKind,
) -> Result<Threshold, DriveFailure> {
    let Some(after_amount) = figure_in(after, scope, kind) else {
        return Err(DriveFailure::PostconditionUnverifiable { kind });
    };

    // Differing units cannot be compared. An uncheckable postcondition is a failure.
    if before.unit() != after_amount.unit() {
        return Err(DriveFailure::PostconditionUnverifiable { kind });
    }

    if kind.size_of(after_amount) > kind.size_of(before) {
        Err(DriveFailure::ExposureIncreasedAfterSafeState { kind, before, after: after_amount })
    } else {
        Ok(after_amount)
    }
}

/// Same scope on both sides: one venue's exposure read against the book's would report a false fall.
pub(crate) fn figure_in(
    snapshot: &ExposureSnapshot,
    scope: &Scope,
    kind: LimitKind,
) -> Option<Threshold> {
    snapshot
        .figures
        .iter()
        .find(|(figure_scope, figure_kind, _)| figure_scope == scope && *figure_kind == kind)
        .map(|(_, _, figure)| *figure)
}
