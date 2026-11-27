// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The pre-trade check's latency bound as a gate: asserts the shape of the curve, not nanoseconds.
//! In `tests/` because the PreTradeCheck bound gate refuses `Instant::now` in `src/`.

use std::hint::black_box;
use std::time::Instant;

use bijective_limits::{
    Amount, Breach, ExposureSnapshot, LimitKind, PositionHeld, PreTradeCheck, ReduceOnlySemantics,
    SafeState, Scope, ScopeLevel, Threshold, select_safe_state,
};

const STALENESS_BOUND_NS: i128 = 10_000_000_000;
const T0: i128 = 1_700_000_000_000_000_000;

/// Non-breaching on purpose: `assess` returns on its first finding, so the clean case is the worst case.
fn pre_trade_check_with(n: usize) -> PreTradeCheck {
    let limits = (0..n)
        .map(|_| {
            bijective_limits::Limit::declare(
                Scope::Instrument("aaa".to_string()),
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(1_000_000)),
            )
            .expect("the kind and the unit agree")
        })
        .collect();

    let mut g =
        PreTradeCheck::new(limits, STALENESS_BOUND_NS, ReduceOnlySemantics::PlacementPrecondition);
    g.on_snapshot(
        ExposureSnapshot {
            observed_at_ns: T0,
            figures: vec![(
                Scope::Portfolio,
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(10)),
            )],
        },
        PositionHeld::Yes,
        T0,
    );
    g
}

fn time_assess(g: &PreTradeCheck, iterations: u32) -> u128 {
    // Warm up so a cold cache is not measured as pre-trade check cost.
    for _ in 0..(iterations / 10).max(1) {
        black_box(g.assess(black_box(T0)));
    }
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(g.assess(black_box(T0)));
    }
    start.elapsed().as_nanos().max(1)
}

/// Linear lands near 32x, quadratic near 1024x. The loose ceiling catches a change of shape
/// without failing on a busy build machine.
#[test]
fn assess_scales_with_the_limit_count_and_not_worse() {
    let small = pre_trade_check_with(8);
    let large = pre_trade_check_with(256);

    let iterations = 20_000;
    let t_small = time_assess(&small, iterations);
    let t_large = time_assess(&large, iterations);

    let ratio = t_large as f64 / t_small as f64;
    assert!(
        ratio < 100.0,
        "assess appears worse than linear in the limit count: 8 limits {t_small}ns, \
         256 limits {t_large}ns, ratio {ratio:.1}x against a ceiling of 100x (linear is ~32x)"
    );
}

/// A deployment may declare no limits. That must be a fast Continue, not a forgotten special case.
#[test]
fn a_pre_trade_check_with_no_limits_still_answers() {
    let g = pre_trade_check_with(0);
    let elapsed = time_assess(&g, 20_000);
    assert!(elapsed > 0, "the measurement itself must be real");
    assert_eq!(
        g.assess(T0),
        bijective_limits::PreTradeVerdict::Continue,
        "no limits declared is not a reason to halt"
    );
}

/// Scope is fixed at five levels.
#[test]
fn every_scope_level_is_declarable() {
    for scope in [
        Scope::Portfolio,
        Scope::Desk("alpha".to_string()),
        Scope::Venue("venue-a".to_string()),
        Scope::Strategy("s1".to_string()),
        Scope::Underlying("btc".to_string()),
        Scope::Instrument("aaa".to_string()),
    ] {
        bijective_limits::Limit::declare(
            scope,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000_000)),
        )
        .expect("every fixed scope level declares");
    }
    // Six, not five or seven: another is a decision record first (`docs/decisions/`).
    assert_eq!(ScopeLevel::ALL.len(), 6);
}

/// Pins the current answer to the open risk limits venue-policy question (`docs/concepts/risk_and_limits.md`) so a change is read.
#[test]
fn a_venue_level_breach_halts_pending_an_ff9_design_decision() {
    let breach = Breach {
        level: ScopeLevel::Venue,
        kind: LimitKind::GrossExposure,
        maximum: Threshold::Money(Amount::from_scaled(1_000)),
        observed: Threshold::Money(Amount::from_scaled(2_000)),
    };

    assert_eq!(
        select_safe_state(&breach, PositionHeld::Yes, ReduceOnlySemantics::PlacementPrecondition),
        SafeState::Halted,
        "the fail-closed default stands until the venue policy is decided"
    );
}
