// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! When the portfolio's figures are worth recording again.
//!
//! The figures move with every mark, so recording each pass would grow the control stream by one
//! entry a second for as long as the node runs. A summary is recorded when a figure appears or
//! disappears, when one has moved by at least one percent of its last recorded size, or when a
//! minute has passed with any change at all. A reader therefore sees every loss of a figure at
//! once, every material move at once, and the rest at most once a minute.

use bijective_limits::{ExposureSnapshot, LimitKind, Scope, Threshold};

use crate::wire::{ScopeFigure, Valued};

/// The longest a changed figure waits to be recorded.
pub const AT_MOST_EVERY_NS: i128 = 60_000_000_000;
/// One percent, as a divisor: a move of a hundredth of the last recorded size is material.
const MATERIAL_DIVISOR: i128 = 100;

/// The portfolio's figures in `snapshot`, with the book's `equity_scaled`, which no limit judges
/// and the snapshot therefore does not hold. A figure the snapshot does not hold is absent.
#[must_use]
pub fn portfolio(snapshot: &ExposureSnapshot, equity_scaled: Option<i128>) -> Valued {
    let figure = |kind: LimitKind| {
        snapshot.figures.iter().find_map(|(scope, k, threshold)| match (scope, threshold) {
            (Scope::Portfolio, Threshold::Money(amount)) if *k == kind => Some(amount.scaled()),
            (Scope::Portfolio, Threshold::Ratio(ratio)) if *k == kind => Some(ratio.scaled()),
            _ => None,
        })
    };
    Valued {
        gross_scaled: figure(LimitKind::GrossExposure),
        net_scaled: figure(LimitKind::NetExposure),
        daily_loss_scaled: figure(LimitKind::DailyLoss),
        observed_at_ns: snapshot.observed_at_ns,
        leverage_scaled: figure(LimitKind::Leverage),
        margin_use_scaled: figure(LimitKind::MarginUse),
        equity_scaled,
    }
}

/// Whether `next` is to be recorded, given the last summary recorded and when.
#[must_use]
pub fn due(last: Option<(&Valued, i128)>, next: &Valued, at_ns: i128) -> bool {
    let Some((last, recorded_at_ns)) = last else { return true };
    let pairs = [
        (last.gross_scaled, next.gross_scaled),
        (last.net_scaled, next.net_scaled),
        (last.daily_loss_scaled, next.daily_loss_scaled),
        (last.leverage_scaled, next.leverage_scaled),
        (last.margin_use_scaled, next.margin_use_scaled),
        (last.equity_scaled, next.equity_scaled),
    ];
    if pairs.iter().any(|(a, b)| a.is_some() != b.is_some()) {
        return true;
    }
    let moved = pairs.iter().filter_map(|(a, b)| Some((a.as_ref()?, b.as_ref()?)));
    let mut changed = false;
    for (was, now) in moved {
        let step = (now - was).abs();
        if step > 0 && step * MATERIAL_DIVISOR >= was.abs().max(1) {
            return true;
        }
        changed |= step > 0;
    }
    changed && at_ns - recorded_at_ns >= AT_MOST_EVERY_NS
}

/// Every money figure below the portfolio in `snapshot`, in the order the snapshot holds them.
#[must_use]
pub fn scoped(snapshot: &ExposureSnapshot) -> Vec<ScopeFigure> {
    snapshot
        .figures
        .iter()
        .filter_map(|(scope, kind, threshold)| match (scope, threshold) {
            (Scope::Portfolio, _) => None,
            (scope, Threshold::Money(amount)) => Some(ScopeFigure {
                scope: scope.clone(),
                kind: *kind,
                value_scaled: amount.scaled(),
            }),
            _ => None,
        })
        .collect()
}

/// Whether `next` is to be recorded, by the portfolio summary's rule: a figure appeared or went,
/// one moved by a hundredth, or a minute passed with any change.
#[must_use]
pub fn scoped_due(last: Option<(&[ScopeFigure], i128)>, next: &[ScopeFigure], at_ns: i128) -> bool {
    let Some((last, recorded_at_ns)) = last else { return !next.is_empty() };
    let key = |f: &ScopeFigure| (f.scope.clone(), f.kind);
    let was: std::collections::BTreeMap<_, i128> =
        last.iter().map(|f| (key(f), f.value_scaled)).collect();
    if was.len() != next.len() || next.iter().any(|f| !was.contains_key(&key(f))) {
        return true;
    }
    let mut changed = false;
    for f in next {
        let before = was[&key(f)];
        let step = (f.value_scaled - before).abs();
        if step > 0 && step * MATERIAL_DIVISOR >= before.abs().max(1) {
            return true;
        }
        changed |= step > 0;
    }
    changed && at_ns - recorded_at_ns >= AT_MOST_EVERY_NS
}

#[cfg(test)]
#[path = "../tests/unit/summary_tests.rs"]
mod tests;
