// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Acceptance gates for the risk limits limit evaluation core. Time is supplied as data.

use super::*;

// A wrapped exposure figure is a breach that silently reads as compliant.

#[test]
fn adding_exposure_past_the_representable_range_refuses_rather_than_wraps() {
    let near_maximum = Amount::from_scaled(i128::MAX - 1);

    let result = near_maximum.add(Amount::from_scaled(2));

    assert_eq!(
        result,
        Err(ArithmeticError::Overflow),
        "exposure arithmetic wrapped instead of refusing"
    );
}

// A limit is a maximum permitted value: exactly at the limit is compliant.

#[test]
fn an_exposure_above_its_limit_is_a_breach() {
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::GrossExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("the kind and the unit agree");

    let outcome = limit.judge(Threshold::Money(Amount::from_scaled(1_001))).unwrap();

    assert_eq!(
        outcome,
        Some(Breach {
            level: ScopeLevel::Portfolio,
            kind: LimitKind::GrossExposure,
            observed: Threshold::Money(Amount::from_scaled(1_001)),
            maximum: Threshold::Money(Amount::from_scaled(1_000)),
        })
    );
}

#[test]
fn an_exposure_exactly_at_its_limit_is_not_a_breach() {
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::GrossExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("the kind and the unit agree");

    let outcome = limit.judge(Threshold::Money(Amount::from_scaled(1_000))).unwrap();

    assert_eq!(outcome, None, "a limit is a maximum permitted, not a ceiling to stay under");
}

#[test]
fn a_breach_carries_the_threshold_it_crossed_and_the_value_that_crossed_it() {
    // The evidence travels with the finding, not reconstructed from logs.
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::Leverage,
        Threshold::Ratio(Ratio::from_scaled(500)),
    )
    .expect("the kind and the unit agree");

    let breach = limit
        .judge(Threshold::Ratio(Ratio::from_scaled(750)))
        .expect("a ratio limit accepts a ratio observation")
        .expect("an observation above its limit is a breach");

    assert_eq!(breach.kind, LimitKind::Leverage);
    assert_eq!(breach.observed, Threshold::Ratio(Ratio::from_scaled(750)));
    assert_eq!(breach.maximum, Threshold::Ratio(Ratio::from_scaled(500)));
}

fn entity(desk: &str, venue: &str, strategy: &str, instrument: &str) -> Entity {
    Entity {
        desk: desk.to_string(),
        venue: venue.to_string(),
        strategy: strategy.to_string(),
        instrument: instrument.to_string(),
    }
}

// A stale evaluation is not a safe one: "nothing is wrong" and "could not tell" stay distinct.

const T0: i128 = 1_800_000_000_000_000_000;
const NS_PER_S: i128 = 1_000_000_000;
const FRESHNESS_BOUND_NS: i128 = NS_PER_S;

/// Shaped as valuation produces it: one figure per scope computed, not one for the whole book.
fn snapshot(observed_at_ns: i128, gross: i128) -> ExposureSnapshot {
    let money = Threshold::Money(Amount::from_scaled(gross));
    ExposureSnapshot {
        observed_at_ns,
        figures: vec![
            (Scope::Portfolio, LimitKind::GrossExposure, money),
            (Scope::Instrument("aaa".to_string()), LimitKind::GrossExposure, money),
        ],
    }
}

#[test]
fn a_clear_evaluation_requires_fresh_state_and_every_limit_judged() {
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
    ];

    let evaluation = evaluate(&limits, &snapshot(T0, 900), T0, FRESHNESS_BOUND_NS);

    assert!(evaluation.is_clear());
    assert!(evaluation.breaches.is_empty());
    assert!(evaluation.unjudged.is_empty());
    assert_eq!(evaluation.freshness, Freshness::Fresh);
}

#[test]
fn no_breach_found_in_stale_state_is_not_clear() {
    // Too old to stand behind: reporting clear would call the book safe on data that stopped arriving.
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
    ];
    let asked_at = T0 + FRESHNESS_BOUND_NS + 1;

    let evaluation = evaluate(&limits, &snapshot(T0, 900), asked_at, FRESHNESS_BOUND_NS);

    assert!(
        !evaluation.is_clear(),
        "stale state without a breach was reported as safe to continue"
    );
    assert!(evaluation.breaches.is_empty());
    assert_eq!(evaluation.freshness, Freshness::Stale { age_ns: FRESHNESS_BOUND_NS + 1 });
}

#[test]
fn a_breach_found_in_stale_state_is_still_reported_and_labelled_stale() {
    // Staleness never suppresses a breach: acting on an old one is recoverable, ignoring it is not.
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
    ];
    let asked_at = T0 + FRESHNESS_BOUND_NS + 1;

    let evaluation = evaluate(&limits, &snapshot(T0, 1_500), asked_at, FRESHNESS_BOUND_NS);

    assert_eq!(
        evaluation.breaches,
        vec![ScopedBreach {
            scope: Scope::Portfolio,
            breach: Breach {
                level: ScopeLevel::Portfolio,
                kind: LimitKind::GrossExposure,
                observed: Threshold::Money(Amount::from_scaled(1_500)),
                maximum: Threshold::Money(Amount::from_scaled(1_000)),
            },
        }]
    );
    assert_eq!(evaluation.freshness, Freshness::Stale { age_ns: FRESHNESS_BOUND_NS + 1 });
}

#[test]
fn state_exactly_at_the_freshness_bound_is_still_fresh() {
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
    ];

    let evaluation =
        evaluate(&limits, &snapshot(T0, 900), T0 + FRESHNESS_BOUND_NS, FRESHNESS_BOUND_NS);

    assert_eq!(evaluation.freshness, Freshness::Fresh);
    assert!(evaluation.is_clear());
}

#[test]
fn a_limit_with_no_matching_figure_is_reported_unjudged_not_satisfied() {
    // A missing figure must not read as a passing one.
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
        Limit::declare(
            Scope::Portfolio,
            LimitKind::Leverage,
            Threshold::Ratio(Ratio::from_scaled(200)),
        )
        .expect("the kind and the unit agree"),
    ];

    let evaluation = evaluate(&limits, &snapshot(T0, 900), T0, FRESHNESS_BOUND_NS);

    assert_eq!(evaluation.unjudged, vec![LimitKind::Leverage]);
    assert!(!evaluation.is_clear(), "a limit with no figure was silently treated as satisfied");
}

#[test]
fn a_breach_and_an_unjudged_limit_are_both_reported() {
    // Breach and unjudged co-occur and need different responses. One result must carry both.
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
        Limit::declare(
            Scope::Portfolio,
            LimitKind::Leverage,
            Threshold::Ratio(Ratio::from_scaled(200)),
        )
        .expect("the kind and the unit agree"),
    ];

    let evaluation = evaluate(&limits, &snapshot(T0, 1_500), T0, FRESHNESS_BOUND_NS);

    assert_eq!(evaluation.breaches.len(), 1, "the breach was lost");
    assert_eq!(evaluation.unjudged, vec![LimitKind::Leverage], "the unjudged limit was lost");
    assert!(!evaluation.is_clear());
}

#[test]
fn state_dated_ahead_of_the_instant_asked_about_is_not_treated_as_fresh() {
    // A future-stamped snapshot must not become a negative age read as fresh. A fast clock would suppress staleness.
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
    ];
    let skew_ns = 5 * NS_PER_S;

    let evaluation = evaluate(&limits, &snapshot(T0 + skew_ns, 900), T0, FRESHNESS_BOUND_NS);

    assert_eq!(evaluation.freshness, Freshness::Implausible { skew_ns });
    assert!(
        !evaluation.is_clear(),
        "state from a disagreeing clock was reported as safe to continue"
    );
}

// Reducing means do not extend existing exposure, not do not create it. A flat instrument is not blocked.

fn scoped(scope: Scope, kind: LimitKind, maximum: i128) -> Limit {
    let threshold = match kind.unit() {
        Unit::Money => Threshold::Money(Amount::from_scaled(maximum)),
        Unit::Ratio => Threshold::Ratio(Ratio::from_scaled(maximum)),
    };
    Limit::declare(scope, kind, threshold).expect("the helper builds a limit in its own unit")
}

fn breach_at(scope: Scope) -> Breach {
    scoped(scope, LimitKind::GrossExposure, 1_000)
        .judge(Threshold::Money(Amount::from_scaled(1_500)))
        .unwrap()
        .expect("an exposure above its limit is a breach")
}

#[test]
fn a_portfolio_scope_breach_drives_halted_because_reducing_would_leave_flat_instruments_open() {
    let state = select_safe_state(
        &breach_at(Scope::Portfolio),
        PositionHeld::Yes,
        ReduceOnlySemantics::PlacementPrecondition,
    );

    assert_eq!(state, SafeState::Halted);
}

#[test]
fn an_instrument_scope_breach_on_a_held_position_drives_reducing() {
    let state = select_safe_state(
        &breach_at(Scope::Instrument("aaa".to_string())),
        PositionHeld::Yes,
        ReduceOnlySemantics::PlacementPrecondition,
    );

    assert_eq!(state, SafeState::Reducing);
}

#[test]
fn an_instrument_scope_breach_with_nothing_held_drives_halted() {
    // Flat instrument: Reducing is false on both arms and the order proceeds, so the breach must escalate.
    let state = select_safe_state(
        &breach_at(Scope::Instrument("aaa".to_string())),
        PositionHeld::No,
        ReduceOnlySemantics::PlacementPrecondition,
    );

    assert_eq!(state, SafeState::Halted, "Reducing was selected where it would stop nothing");
}

#[test]
fn a_venue_that_clamps_at_fill_time_drives_halted_even_on_a_held_position() {
    // Fill-time clamp: reduce-only against no position still fills, so Reducing guarantees nothing here.
    let state = select_safe_state(
        &breach_at(Scope::Instrument("aaa".to_string())),
        PositionHeld::Yes,
        ReduceOnlySemantics::FillTimeClamp,
    );

    assert_eq!(state, SafeState::Halted);
}

#[test]
fn a_desk_scope_breach_drives_halted() {
    // Desk and strategy scope span many instruments, mostly flat, so the portfolio reasoning applies.
    assert_eq!(
        select_safe_state(
            &breach_at(Scope::Desk("alpha".to_string())),
            PositionHeld::Yes,
            ReduceOnlySemantics::PlacementPrecondition,
        ),
        SafeState::Halted
    );
}

// No figure received is not evidence that nothing is wrong. It is evidence that nothing is being checked.

const STALENESS_BOUND_NS: i128 = 10 * NS_PER_S;

fn pre_trade_check() -> PreTradeCheck {
    PreTradeCheck::new(
        vec![
            Limit::declare(
                Scope::Instrument("aaa".to_string()),
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(1_000)),
            )
            .expect("the kind and the unit agree"),
        ],
        STALENESS_BOUND_NS,
        ReduceOnlySemantics::PlacementPrecondition,
    )
}

#[test]
fn a_pre_trade_check_that_has_never_been_primed_halts() {
    // Startup window: a pre-trade check holding nothing must not return carry on.
    let verdict = pre_trade_check().assess(T0);

    assert_eq!(
        verdict,
        PreTradeVerdict::Drive { state: SafeState::Halted, reason: SafeStateReason::NeverPrimed },
        "an unprimed pre_trade_check allowed trading to continue"
    );
}

#[test]
fn stale_inputs_past_the_declared_bound_halts() {
    let mut pre_trade_check = pre_trade_check();
    pre_trade_check.on_snapshot(snapshot(T0, 500), PositionHeld::Yes, T0);

    let verdict = pre_trade_check.assess(T0 + STALENESS_BOUND_NS + 1);

    assert_eq!(
        verdict,
        PreTradeVerdict::Drive {
            state: SafeState::Halted,
            reason: SafeStateReason::InputsStale { stale_for_ns: STALENESS_BOUND_NS + 1 },
        }
    );
}

#[test]
fn an_input_exactly_at_the_bound_is_not_yet_stale() {
    let mut pre_trade_check = pre_trade_check();
    pre_trade_check.on_snapshot(snapshot(T0, 500), PositionHeld::Yes, T0);

    let verdict = pre_trade_check.assess(T0 + STALENESS_BOUND_NS);

    assert_eq!(verdict, PreTradeVerdict::Continue);
}

#[test]
fn a_breach_in_held_state_drives_the_state_that_breach_calls_for() {
    let mut pre_trade_check = pre_trade_check();
    pre_trade_check.on_snapshot(snapshot(T0, 1_500), PositionHeld::Yes, T0);

    let verdict = pre_trade_check.assess(T0);

    assert_eq!(
        verdict,
        PreTradeVerdict::Drive {
            state: SafeState::Reducing,
            reason: SafeStateReason::Breached(Breach {
                level: ScopeLevel::Instrument,
                kind: LimitKind::GrossExposure,
                observed: Threshold::Money(Amount::from_scaled(1_500)),
                maximum: Threshold::Money(Amount::from_scaled(1_000)),
            }),
        }
    );
}

#[test]
fn stale_inputs_outrank_a_clean_reading_that_is_out_of_date() {
    // The held figures are the last received before the feed stopped. They say nothing about now.
    let mut pre_trade_check = pre_trade_check();
    pre_trade_check.on_snapshot(snapshot(T0, 500), PositionHeld::Yes, T0);

    let verdict = pre_trade_check.assess(T0 + STALENESS_BOUND_NS + 1);

    match verdict {
        PreTradeVerdict::Drive { reason, .. } => assert!(
            matches!(reason, SafeStateReason::InputsStale { .. }),
            "a stale clean reading was preferred over reporting the stale input"
        ),
        PreTradeVerdict::Continue => panic!("a stale input was treated as a passing reading"),
    }
}

#[test]
fn a_limit_the_held_snapshot_cannot_judge_halts_rather_than_being_approximated() {
    // A figure the input source has not pushed is not satisfied and not guessable.
    let mut pre_trade_check = PreTradeCheck::new(
        vec![
            Limit::declare(
                Scope::Portfolio,
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(1_000)),
            )
            .expect("the kind and the unit agree"),
            Limit::declare(
                Scope::Portfolio,
                LimitKind::Leverage,
                Threshold::Ratio(Ratio::from_scaled(200)),
            )
            .expect("the kind and the unit agree"),
        ],
        STALENESS_BOUND_NS,
        ReduceOnlySemantics::PlacementPrecondition,
    );
    pre_trade_check.on_snapshot(snapshot(T0, 500), PositionHeld::Yes, T0);

    let verdict = pre_trade_check.assess(T0);

    assert_eq!(
        verdict,
        PreTradeVerdict::Drive {
            state: SafeState::Halted,
            reason: SafeStateReason::Unjudgeable { kind: LimitKind::Leverage },
        },
        "a limit with no figure was treated as satisfied inside the hand"
    );
}

#[test]
fn a_fresh_snapshot_clears_earlier_stale_inputs() {
    // Not latched: once the input source speaks again, release is the input source's decision with the operator.
    let mut pre_trade_check = pre_trade_check();
    pre_trade_check.on_snapshot(snapshot(T0, 500), PositionHeld::Yes, T0);
    assert!(matches!(
        pre_trade_check.assess(T0 + STALENESS_BOUND_NS + 1),
        PreTradeVerdict::Drive { .. }
    ));

    let later = T0 + STALENESS_BOUND_NS + 2;
    pre_trade_check.on_snapshot(snapshot(later, 500), PositionHeld::Yes, later);

    assert_eq!(pre_trade_check.assess(later), PreTradeVerdict::Continue);
}

// The driver asserts the postcondition: a fill-time clamping venue accepts an instruction it does not honour.

/// A control that applies what it is told. Stands in for a healthy substrate.
struct WorkingControl {
    state: SafeState,
}

impl TradingStateControl for WorkingControl {
    fn set(&mut self, state: SafeState) {
        self.state = state;
    }
    fn current(&self) -> SafeState {
        self.state
    }
}

/// Accepts every call and applies none, like a stale setter handle or a node mid-shutdown.
struct IgnoringControl {
    state: SafeState,
}

impl TradingStateControl for IgnoringControl {
    fn set(&mut self, _state: SafeState) {}
    fn current(&self) -> SafeState {
        self.state
    }
}

#[test]
fn a_state_that_takes_effect_is_reported_applied() {
    let mut control = WorkingControl { state: SafeState::Active };

    let outcome = drive_safe_state(&mut control, SafeState::Halted);

    assert_eq!(outcome, Ok(()));
    assert_eq!(control.current(), SafeState::Halted);
}

#[test]
fn a_state_change_that_does_not_take_effect_is_detected() {
    // The call returned and the platform is still trading. Without the read-back this halt is only assumed.
    let mut control = IgnoringControl { state: SafeState::Active };

    let outcome = drive_safe_state(&mut control, SafeState::Halted);

    assert_eq!(
        outcome,
        Err(DriveFailure::NotApplied { requested: SafeState::Halted, observed: SafeState::Active }),
        "a safe state that never took effect was reported as applied"
    );
}

#[test]
fn exposure_rising_after_a_safe_state_was_driven_is_a_postcondition_failure() {
    // Applied and exposure grew anyway, as under a fill-time reduce-only clamp.
    let before = Threshold::Money(Amount::from_scaled(1_500));
    let after = snapshot(T0 + NS_PER_S, 1_800);

    let outcome = verify_exposure_did_not_increase(
        before,
        &after,
        &Scope::Portfolio,
        LimitKind::GrossExposure,
    );

    assert_eq!(
        outcome,
        Err(DriveFailure::ExposureIncreasedAfterSafeState {
            kind: LimitKind::GrossExposure,
            before: Threshold::Money(Amount::from_scaled(1_500)),
            after: Threshold::Money(Amount::from_scaled(1_800)),
        })
    );
}

#[test]
fn exposure_holding_or_falling_after_a_safe_state_satisfies_the_postcondition() {
    let before = Threshold::Money(Amount::from_scaled(1_500));
    let unchanged = snapshot(T0 + NS_PER_S, 1_500);
    let reduced = snapshot(T0 + NS_PER_S, 900);

    assert_eq!(
        verify_exposure_did_not_increase(
            before,
            &unchanged,
            &Scope::Portfolio,
            LimitKind::GrossExposure
        ),
        Ok(Threshold::Money(Amount::from_scaled(1_500)))
    );
    assert_eq!(
        verify_exposure_did_not_increase(
            before,
            &reduced,
            &Scope::Portfolio,
            LimitKind::GrossExposure
        ),
        Ok(Threshold::Money(Amount::from_scaled(900)))
    );
}

#[test]
fn a_postcondition_that_cannot_be_checked_is_a_failure_not_a_pass() {
    // No later figure: whether exposure grew is unknown, not satisfied.
    let before = Threshold::Money(Amount::from_scaled(1_500));
    let after = ExposureSnapshot { observed_at_ns: T0 + NS_PER_S, figures: Vec::new() };

    let outcome = verify_exposure_did_not_increase(
        before,
        &after,
        &Scope::Portfolio,
        LimitKind::GrossExposure,
    );

    assert_eq!(
        outcome,
        Err(DriveFailure::PostconditionUnverifiable { kind: LimitKind::GrossExposure }),
        "a postcondition that could not be checked was reported as satisfied"
    );
}

// Provenance is written whether or not the alert is delivered. Safety is never gated on recording.

/// A sink that accepts records. Stands in for a healthy store.
struct AcceptingSink {
    written: Vec<ProvenanceRecord>,
}

impl ProvenanceSink for AcceptingSink {
    fn append(&mut self, record: ProvenanceRecord) -> Result<(), SinkUnavailable> {
        self.written.push(record);
        Ok(())
    }
}

/// A sink that is down.
struct UnavailableSink;

impl ProvenanceSink for UnavailableSink {
    fn append(&mut self, _record: ProvenanceRecord) -> Result<(), SinkUnavailable> {
        Err(SinkUnavailable)
    }
}

fn record_at(at_ns: i128, observed: i128) -> ProvenanceRecord {
    ProvenanceRecord {
        at_ns,
        reason: SafeStateReason::Breached(Breach {
            level: ScopeLevel::Instrument,
            kind: LimitKind::GrossExposure,
            observed: Threshold::Money(Amount::from_scaled(observed)),
            maximum: Threshold::Money(Amount::from_scaled(1_000)),
        }),
        state_driven: SafeState::Reducing,
        freshness: Freshness::Fresh,
    }
}

#[test]
fn a_record_carries_the_rule_the_value_the_threshold_and_how_live_the_inputs_were() {
    let mut sink = AcceptingSink { written: Vec::new() };
    let mut recorder = Recorder::new(4);

    let outcome = recorder.record(&mut sink, record_at(T0, 1_500));

    assert_eq!(outcome, RecordOutcome::Written);
    assert_eq!(sink.written, vec![record_at(T0, 1_500)]);
}

#[test]
fn a_record_the_store_could_not_take_is_retained_not_lost() {
    // The record is held and the caller told it is unwritten, so the gap is known while it exists.
    let mut sink = UnavailableSink;
    let mut recorder = Recorder::new(4);

    let outcome = recorder.record(&mut sink, record_at(T0, 1_500));

    assert_eq!(outcome, RecordOutcome::Retained { pending: 1 });
    assert_eq!(recorder.pending(), 1);
    assert_eq!(recorder.dropped(), 0);
}

#[test]
fn retention_is_bounded_and_keeps_the_onset_of_an_incident() {
    // Newest is dropped, not oldest: provenance exists to preserve the onset, and the count states the gap.
    let mut sink = UnavailableSink;
    let mut recorder = Recorder::new(2);

    recorder.record(&mut sink, record_at(T0, 1_100));
    recorder.record(&mut sink, record_at(T0 + 1, 1_200));
    let overflow = recorder.record(&mut sink, record_at(T0 + 2, 1_300));

    assert_eq!(overflow, RecordOutcome::Dropped { dropped_total: 1 });
    assert_eq!(recorder.pending(), 2);
    assert_eq!(recorder.dropped(), 1);
}

#[test]
fn a_dropped_record_is_counted_never_silently_discarded() {
    // The count is the evidence that evidence is missing.
    let mut sink = UnavailableSink;
    let mut recorder = Recorder::new(1);

    recorder.record(&mut sink, record_at(T0, 1_100));
    recorder.record(&mut sink, record_at(T0 + 1, 1_200));
    recorder.record(&mut sink, record_at(T0 + 2, 1_300));

    assert_eq!(recorder.dropped(), 2, "records vanished without being counted");
}

#[test]
fn flushing_after_the_store_recovers_writes_the_retained_records_in_order() {
    let mut down = UnavailableSink;
    let mut recorder = Recorder::new(4);
    recorder.record(&mut down, record_at(T0, 1_100));
    recorder.record(&mut down, record_at(T0 + 1, 1_200));

    let mut up = AcceptingSink { written: Vec::new() };
    let flushed = recorder.flush(&mut up);

    assert_eq!(flushed, Ok(2));
    assert_eq!(
        up.written,
        vec![record_at(T0, 1_100), record_at(T0 + 1, 1_200)],
        "retained records were written out of the order they occurred"
    );
    assert_eq!(recorder.pending(), 0);
}

#[test]
fn a_flush_that_fails_partway_keeps_what_it_could_not_write() {
    let mut down = UnavailableSink;
    let mut recorder = Recorder::new(4);
    recorder.record(&mut down, record_at(T0, 1_100));

    let flushed = recorder.flush(&mut down);

    assert_eq!(flushed, Err(SinkUnavailable));
    assert_eq!(recorder.pending(), 1, "a record was lost by a failed flush");
}

// Determinism: one hash-ordered collection in the evaluator would let two runs disagree on the first breach.

#[test]
fn the_same_inputs_produce_an_identical_evaluation_every_time() {
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
        )
        .expect("the kind and the unit agree"),
        Limit::declare(
            Scope::Desk("alpha".to_string()),
            LimitKind::NetExposure,
            Threshold::Money(Amount::from_scaled(400)),
        )
        .expect("the kind and the unit agree"),
        Limit::declare(
            Scope::Instrument("aaa".to_string()),
            LimitKind::Concentration,
            Threshold::Money(Amount::from_scaled(100)),
        )
        .expect("the kind and the unit agree"),
        Limit::declare(
            Scope::Portfolio,
            LimitKind::Leverage,
            Threshold::Ratio(Ratio::from_scaled(50)),
        )
        .expect("the kind and the unit agree"),
    ];
    // Each figure at its limit's scope: a desk limit was once judged against the book's net.
    let state = ExposureSnapshot {
        observed_at_ns: T0,
        figures: vec![
            (
                Scope::Portfolio,
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(1_500)),
            ),
            (
                Scope::Desk("alpha".to_string()),
                LimitKind::NetExposure,
                Threshold::Money(Amount::from_scaled(900)),
            ),
            (
                Scope::Instrument("aaa".to_string()),
                LimitKind::Concentration,
                Threshold::Money(Amount::from_scaled(20)),
            ),
        ],
    };

    let first = evaluate(&limits, &state, T0, FRESHNESS_BOUND_NS);
    for _ in 0..64 {
        assert_eq!(
            evaluate(&limits, &state, T0, FRESHNESS_BOUND_NS),
            first,
            "two evaluations of identical inputs disagreed"
        );
    }

    // Order matters: a different finding sequence reaches a different intermediate state.
    assert_eq!(first.breaches.len(), 2);
    assert_eq!(first.breaches[0].breach.kind, LimitKind::GrossExposure);
    assert_eq!(first.breaches[1].breach.kind, LimitKind::NetExposure);
    assert_eq!(first.unjudged, vec![LimitKind::Leverage]);
}

// Units are declared, never inferred: a kill switch with percent thresholds against a fractional value never fired.

#[test]
fn a_money_limit_declared_with_a_ratio_threshold_is_refused() {
    // Gross exposure is money. A ratio declaration is refused where made, not where it silently fails to fire.
    let result = Limit::declare(
        Scope::Portfolio,
        LimitKind::GrossExposure,
        Threshold::Ratio(Ratio::from_scaled(5_000)),
    );

    assert_eq!(
        result,
        Err(LimitError::UnitMismatch {
            kind: LimitKind::GrossExposure,
            expected: Unit::Money,
            declared: Unit::Ratio,
        })
    );
}

#[test]
fn a_ratio_limit_declared_with_a_money_threshold_is_refused() {
    let result = Limit::declare(
        Scope::Portfolio,
        LimitKind::Leverage,
        Threshold::Money(Amount::from_scaled(500)),
    );

    assert_eq!(
        result,
        Err(LimitError::UnitMismatch {
            kind: LimitKind::Leverage,
            expected: Unit::Ratio,
            declared: Unit::Money,
        })
    );
}

#[test]
fn each_limit_kind_states_the_unit_it_is_measured_in() {
    assert_eq!(LimitKind::GrossExposure.unit(), Unit::Money);
    assert_eq!(LimitKind::NetExposure.unit(), Unit::Money);
    assert_eq!(LimitKind::Concentration.unit(), Unit::Money);
    assert_eq!(LimitKind::DailyLoss.unit(), Unit::Money);
    assert_eq!(LimitKind::Leverage.unit(), Unit::Ratio);
    assert_eq!(LimitKind::Drawdown.unit(), Unit::Money);
}

#[test]
fn an_observation_in_the_wrong_unit_is_refused_rather_than_compared() {
    // A comparison that runs and means nothing must refuse, not return no breach.
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::Leverage,
        Threshold::Ratio(Ratio::from_scaled(500)),
    )
    .expect("a ratio kind accepts a ratio threshold");

    let outcome = limit.judge(Threshold::Money(Amount::from_scaled(-5)));

    assert_eq!(
        outcome,
        Err(LimitError::UnitMismatch {
            kind: LimitKind::Leverage,
            expected: Unit::Ratio,
            declared: Unit::Money,
        }),
        "a cross-unit comparison returned a verdict instead of refusing"
    );
}

#[test]
fn a_correctly_declared_limit_judges_normally() {
    let limit = Limit::declare(
        Scope::Instrument("aaa".to_string()),
        LimitKind::GrossExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("a money kind accepts a money threshold");

    assert!(limit.judge(Threshold::Money(Amount::from_scaled(1_500))).unwrap().is_some());
    assert!(limit.judge(Threshold::Money(Amount::from_scaled(1_000))).unwrap().is_none());
}

// A limit that cannot fire shows on the register as protection. Reachability is proven by a witness breach.

#[test]
fn a_limit_exhibits_an_observation_that_would_breach_it() {
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::GrossExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("valid declaration");

    let witness = limit.witness_breach().expect("a reachable limit exhibits a witness");

    assert!(
        limit.judge(witness).unwrap().is_some(),
        "the witness a limit produced did not breach that same limit"
    );
}

#[test]
fn a_threshold_at_the_representable_ceiling_is_refused_as_unreachable() {
    // Nothing exceeds the largest representable value, so such a limit could never fire.
    let result = Limit::declare(
        Scope::Portfolio,
        LimitKind::GrossExposure,
        Threshold::Money(Amount::from_scaled(i128::MAX)),
    );

    assert_eq!(
        result,
        Err(LimitError::Unreachable { kind: LimitKind::GrossExposure }),
        "a limit that nothing can exceed was accepted"
    );
}

#[test]
fn every_kind_of_correctly_declared_limit_is_reachable() {
    let cases = [
        (Scope::Portfolio, LimitKind::GrossExposure, Threshold::Money(Amount::from_scaled(1_000))),
        (Scope::Portfolio, LimitKind::NetExposure, Threshold::Money(Amount::from_scaled(500))),
        (Scope::Portfolio, LimitKind::Concentration, Threshold::Money(Amount::from_scaled(250))),
        (Scope::Portfolio, LimitKind::DailyLoss, Threshold::Money(Amount::from_scaled(100))),
        (Scope::Portfolio, LimitKind::Leverage, Threshold::Ratio(Ratio::from_scaled(30_000))),
        (Scope::Portfolio, LimitKind::Leverage, Threshold::Ratio(Ratio::from_scaled(500))),
    ];

    for (scope, kind, threshold) in cases {
        let limit = Limit::declare(scope, kind, threshold)
            .unwrap_or_else(|_| panic!("{kind:?} should accept its own unit"));
        let witness = limit
            .witness_breach()
            .unwrap_or_else(|| panic!("{kind:?} produced no witness and is unreachable"));
        assert!(
            limit.judge(witness).unwrap().is_some(),
            "{kind:?} did not fire on its own witness"
        );
    }
}

// Every entry point must carry the unit. A safe constructor beside an unchecked one closes nothing.

/// The constructor the engine calls. `main` refuses to start on it.
#[test]
fn a_declaration_whose_threshold_is_in_the_wrong_unit_is_refused() {
    assert_eq!(
        Limit::declare(
            Scope::Portfolio,
            LimitKind::Leverage,
            Threshold::Money(Amount::from_scaled(500)),
        ),
        Err(LimitError::UnitMismatch {
            kind: LimitKind::Leverage,
            expected: Unit::Ratio,
            declared: Unit::Money,
        }),
        "a mis-declared unit became a limit the platform would judge against"
    );
}

#[test]
fn a_declaration_that_can_never_fire_is_refused() {
    assert_eq!(
        Limit::declare(
            Scope::Portfolio,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(i128::MAX)),
        ),
        Err(LimitError::Unreachable { kind: LimitKind::GrossExposure })
    );
}

#[test]
fn an_evaluation_refuses_a_figure_supplied_in_the_wrong_unit() {
    // A money figure for a ratio limit must be reported unjudgeable, not compared.
    let limits = vec![
        Limit::declare(
            Scope::Portfolio,
            LimitKind::Leverage,
            Threshold::Ratio(Ratio::from_scaled(500)),
        )
        .expect("valid"),
    ];
    let snapshot = ExposureSnapshot {
        observed_at_ns: T0,
        figures: vec![(
            Scope::Portfolio,
            LimitKind::Leverage,
            Threshold::Money(Amount::from_scaled(9_999)),
        )],
    };

    let evaluation = evaluate(&limits, &snapshot, T0, FRESHNESS_BOUND_NS);

    assert!(evaluation.breaches.is_empty());
    assert_eq!(
        evaluation.unjudged,
        vec![LimitKind::Leverage],
        "a cross-unit figure was compared instead of reported unjudgeable"
    );
    assert!(!evaluation.is_clear());
}

#[test]
fn a_pre_trade_check_refuses_a_held_figure_in_the_wrong_unit_rather_than_passing_it() {
    let mut pre_trade_check = PreTradeCheck::new(
        vec![
            Limit::declare(
                Scope::Instrument("aaa".to_string()),
                LimitKind::Leverage,
                Threshold::Ratio(Ratio::from_scaled(30_000)),
            )
            .expect("valid"),
        ],
        STALENESS_BOUND_NS,
        ReduceOnlySemantics::PlacementPrecondition,
    );
    pre_trade_check.on_snapshot(
        ExposureSnapshot {
            observed_at_ns: T0,
            figures: vec![(
                Scope::Portfolio,
                LimitKind::Leverage,
                Threshold::Money(Amount::from_scaled(1)),
            )],
        },
        PositionHeld::Yes,
        T0,
    );

    let verdict = pre_trade_check.assess(T0);

    assert_eq!(
        verdict,
        PreTradeVerdict::Drive {
            state: SafeState::Halted,
            reason: SafeStateReason::Unjudgeable { kind: LimitKind::Leverage },
        },
        "the hand compared across units instead of halting"
    );
}

// The notional, per desk admission

/// The product moves from the instrument's scales to the platform's.
#[test]
fn a_notional_arrives_at_the_platform_scale() {
    // 1.5 units at 2.00, quoted at quantity scale 3 and price scale 2.
    let cost = notional(1_500, 3, 200, 2).expect("representable");

    // 3.00 at scale 8.
    assert_eq!(cost, Amount::from_scaled(300_000_000));
}

/// Rounded up, never down: a short reservation lets spend exceed the declared limit.
#[test]
fn a_remainder_rounds_away_from_zero() {
    // Scales sum eight places past the platform's, with a remainder.
    let cost = notional(1, 12, 1, 4).expect("representable");

    assert_eq!(
        cost,
        Amount::from_scaled(1),
        "a cost smaller than the platform can express is reserved as the smallest it can"
    );
}

#[test]
fn a_negative_remainder_rounds_away_from_zero_too() {
    let cost = notional(-1, 12, 1, 4).expect("representable");

    assert_eq!(cost, Amount::from_scaled(-1));
}

/// A product that cannot be represented is refused, never wrapped.
#[test]
fn an_unrepresentable_notional_is_refused() {
    assert_eq!(notional(i128::MAX, 0, 2, 0), Err(ArithmeticError::Overflow));
    assert_eq!(notional(1, 0, 1, 0).map(|a| a.scaled()), Ok(100_000_000));
    assert_eq!(notional(i128::MAX / 2, 0, 1, 0), Err(ArithmeticError::Overflow));
}

#[test]
fn the_platform_scale_is_named_rather_than_implied() {
    assert_eq!(Amount::SCALE, 8);
    assert_eq!(notional(1_000, 3, 1_000, 3).unwrap(), Amount::from_scaled(100_000_000));
}

/// Two offsetting venues net near zero while one sits at its maximum. Matched by kind alone it read satisfied.
#[test]
fn a_venue_limit_is_not_judged_against_the_books_figure() {
    let venue = Limit::declare(
        Scope::Venue("venue-a".to_string()),
        LimitKind::NetExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("the kind and the unit agree");

    // The book nets to nothing. venue-a alone is well past its limit.
    let offsetting = ExposureSnapshot {
        observed_at_ns: T0,
        figures: vec![
            (Scope::Portfolio, LimitKind::NetExposure, Threshold::Money(Amount::from_scaled(0))),
            (
                Scope::Venue("venue-a".to_string()),
                LimitKind::NetExposure,
                Threshold::Money(Amount::from_scaled(5_000)),
            ),
        ],
    };

    let verdict = evaluate(&[venue], &offsetting, T0, FRESHNESS_BOUND_NS);
    assert_eq!(verdict.breaches.len(), 1, "the venue's own figure was not the one judged");
    assert!(verdict.unjudged.is_empty());
}

#[test]
fn a_scope_with_no_figure_is_unjudged_rather_than_borrowing_one() {
    let desk = Limit::declare(
        Scope::Desk("alpha".to_string()),
        LimitKind::NetExposure,
        Threshold::Money(Amount::from_scaled(10)),
    )
    .expect("the kind and the unit agree");

    // A venue reports what is held, never which desk opened it, so no desk figure exists.
    let book_only = ExposureSnapshot {
        observed_at_ns: T0,
        figures: vec![(
            Scope::Portfolio,
            LimitKind::NetExposure,
            Threshold::Money(Amount::from_scaled(9_000)),
        )],
    };

    let verdict = evaluate(&[desk], &book_only, T0, FRESHNESS_BOUND_NS);
    assert!(verdict.breaches.is_empty(), "a desk limit was judged against the book");
    assert_eq!(verdict.unjudged, vec![LimitKind::NetExposure]);
    assert!(!verdict.is_clear(), "unjudged is not clear");
}

#[test]
fn a_drive_failure_says_how_far_exposure_rose() {
    let failure = DriveFailure::ExposureIncreasedAfterSafeState {
        kind: LimitKind::GrossExposure,
        before: Threshold::Money(Amount::from_scaled(100)),
        after: Threshold::Money(Amount::from_scaled(140)),
    };
    let said = failure.to_string();
    assert!(said.contains("100"), "the figure before is missing: {said}");
    assert!(said.contains("140"), "the figure after is missing: {said}");
}

#[test]
fn a_unit_mismatch_says_what_was_expected_and_not_only_what_was_given() {
    let said = LimitError::UnitMismatch {
        kind: LimitKind::Concentration,
        expected: Unit::Ratio,
        declared: Unit::Money,
    }
    .to_string();
    assert!(said.contains("Ratio"), "the expected unit is missing: {said}");
    assert!(said.contains("Money"), "the declared unit is missing: {said}");
}

#[test]
fn a_dropped_record_says_the_running_total() {
    let said = RecordOutcome::Dropped { dropped_total: 17 }.to_string();
    assert!(said.contains("17"), "the total is missing: {said}");
}

#[test]
fn an_entity_names_every_coordinate_that_was_configured() {
    let said = entity("alpha", "venue-a", "carry", "BTCUSDT").to_string();
    for part in ["alpha", "venue-a", "carry", "BTCUSDT"] {
        assert!(said.contains(part), "{part} is missing from: {said}");
    }
}

#[test]
fn a_drive_that_was_not_applied_says_what_was_asked_for() {
    let said =
        DriveFailure::NotApplied { requested: SafeState::Halted, observed: SafeState::Active }
            .to_string();
    assert!(said.contains("Halted"), "the requested state is missing: {said}");
    assert!(said.contains("Active"), "the observed state is missing: {said}");
}

/// `judge` once carried only the level, so a venue-level breach could not say which venue.
#[test]
fn a_breach_names_the_scope_it_was_about() {
    let limit = Limit::declare(
        Scope::Venue("venue-a".into()),
        LimitKind::GrossExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("a declarable limit");

    let snapshot = ExposureSnapshot {
        observed_at_ns: 10,
        figures: vec![(
            Scope::Venue("venue-a".into()),
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_500)),
        )],
    };
    let evaluation = evaluate(&[limit], &snapshot, 10, 5_000_000_000);

    assert_eq!(evaluation.breaches.len(), 1);
    assert_eq!(evaluation.breaches[0].scope, Scope::Venue("venue-a".into()));
    assert_eq!(evaluation.breaches[0].breach.kind, LimitKind::GrossExposure);
}

/// A net short is as far from flat as a net long of the same size: judged signed, every short
/// would pass a net limit whatever its size.
#[test]
fn a_net_short_beyond_the_maximum_is_a_breach() {
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::NetExposure,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("the kind and the unit agree");

    let short = limit.judge(Threshold::Money(Amount::from_scaled(-1_001))).expect("judged");
    let breach = short.expect("a short past the maximum breaches");
    assert_eq!(breach.observed, Threshold::Money(Amount::from_scaled(-1_001)), "the sign is kept");
    assert_eq!(limit.judge(Threshold::Money(Amount::from_scaled(-1_000))), Ok(None));
}

/// A day that gained is no loss: only net exposure is judged by magnitude.
#[test]
fn a_negative_daily_loss_is_a_gain_and_never_a_breach() {
    let limit = Limit::declare(
        Scope::Portfolio,
        LimitKind::DailyLoss,
        Threshold::Money(Amount::from_scaled(1_000)),
    )
    .expect("the kind and the unit agree");

    assert_eq!(limit.judge(Threshold::Money(Amount::from_scaled(-5_000))), Ok(None));
}

/// A short that grew after a safe state increased exposure, as a long that grew would.
#[test]
fn a_net_short_that_grew_after_a_safe_state_is_an_increase() {
    let after = ExposureSnapshot {
        observed_at_ns: T0,
        figures: vec![(
            Scope::Portfolio,
            LimitKind::NetExposure,
            Threshold::Money(Amount::from_scaled(-2_000)),
        )],
    };

    let checked = verify_exposure_did_not_increase(
        Threshold::Money(Amount::from_scaled(-1_500)),
        &after,
        &Scope::Portfolio,
        LimitKind::NetExposure,
    );
    assert!(
        matches!(checked, Err(DriveFailure::ExposureIncreasedAfterSafeState { .. })),
        "{checked:?}"
    );
}
