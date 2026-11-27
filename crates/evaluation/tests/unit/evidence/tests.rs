// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a verdict must refuse to claim.

use super::*;
use crate::evidence::preregistration::Horizon;
use bijective_strategy_engine::runs::Window;

fn fold(learn: (u64, u64), test: (u64, u64)) -> Fold {
    Fold { learn: Positions::new(learn.0, learn.1), test: Positions::new(test.0, test.1) }
}

/// Two-sided 1.96 in the fixed scale. A test constant, never a source default.
const DECLARED: i128 = 19_600;

#[test]
fn a_walk_forward_over_ordered_disjoint_folds_declares() {
    let w = WalkForward::declare(
        vec![fold((0, 1_000), (1_000, 1_500)), fold((0, 1_500), (1_500, 2_000))],
        Window::ToPosition(2_000),
    )
    .expect("ordered, disjoint and inside the window");

    assert_eq!(w.folds().len(), 2);
    assert_eq!(w.held_out_positions(), 1_000, "the held-out total is the tests alone");
}

#[test]
fn a_test_that_begins_before_its_learn_ends_is_refused() {
    let refused =
        WalkForward::declare(vec![fold((0, 1_000), (900, 1_500))], Window::ToPosition(2_000))
            .expect_err("a rule was tested on history it had already seen");

    assert!(
        matches!(refused, WalkForwardError::TestReachesBackwards { fold: 0, .. }),
        "{refused:?}"
    );
}

#[test]
fn two_tests_covering_the_same_history_are_refused() {
    let refused = WalkForward::declare(
        vec![fold((0, 1_000), (1_000, 1_800)), fold((0, 1_200), (1_500, 2_000))],
        Window::ToPosition(2_000),
    )
    .expect_err("one stretch of history was counted twice");

    assert!(
        matches!(refused, WalkForwardError::TestsOverlap { earlier: 0, later: 1, .. }),
        "{refused:?}"
    );
}

#[test]
fn a_fold_reaching_past_the_runs_window_is_refused() {
    let refused =
        WalkForward::declare(vec![fold((0, 1_000), (1_000, 5_000))], Window::ToPosition(2_000))
            .expect_err("the fold read past the window");

    assert!(
        matches!(
            refused,
            WalkForwardError::OutsideWindow { fold: 0, reaches: 5_000, window_ends: 2_000 }
        ),
        "{refused:?}"
    );
}

#[test]
fn a_structure_with_no_folds_holds_nothing_out() {
    assert_eq!(
        WalkForward::declare(Vec::new(), Window::ToPosition(2_000)).expect_err("nothing held out"),
        WalkForwardError::NoFolds
    );
}

#[test]
fn a_segment_that_reads_nothing_is_refused() {
    let refused =
        WalkForward::declare(vec![fold((1_000, 1_000), (1_000, 1_500))], Window::ToPosition(2_000))
            .expect_err("learned from nothing");

    assert!(
        matches!(refused, WalkForwardError::EmptySegment { fold: 0, segment: "learn" }),
        "{refused:?}"
    );
}

/// Overlapping learn segments are what walk-forward is, and must pass.
#[test]
fn learn_segments_that_overlap_are_the_structure_working() {
    assert!(
        WalkForward::declare(
            vec![
                fold((0, 1_000), (1_000, 1_200)),
                fold((0, 1_200), (1_200, 1_400)),
                fold((0, 1_400), (1_400, 1_600)),
            ],
            Window::ToPosition(1_600),
        )
        .is_ok(),
        "each fold learning from everything before it was treated as a fault"
    );
}

/// (1.96 x 2)^2 = 15.3664, so sixteen. The expected value is written out, not recomputed.
#[test]
fn the_requirement_is_the_squared_ratio_of_noise_to_effect() {
    assert_eq!(observations_required(DECLARED, 2 * ONE, ONE).expect("computable"), 16);
}

#[test]
fn less_noise_needs_less_evidence_and_more_noise_needs_more() {
    let quiet = observations_required(DECLARED, ONE, ONE).expect("computable");
    let loud = observations_required(DECLARED, 4 * ONE, ONE).expect("computable");
    assert!(quiet < loud, "{quiet} was not fewer than {loud}");
    assert_eq!(quiet, 4, "(1.96)^2 rounds up to 4");
}

/// Rounding that went the other way would call an underpowered result sufficient.
#[test]
fn a_requirement_with_a_remainder_rounds_towards_more_evidence() {
    // 1.96 x 1.5 = 2.94, squared is 8.6436. Truncation would ask for eight.
    let required = observations_required(DECLARED, 15_000, ONE).expect("computable");
    assert_eq!(required, 9, "the requirement was rounded towards asking for less");
}

#[test]
fn a_claimed_effect_of_zero_is_not_sizeable_at_any_sample() {
    assert_eq!(
        observations_required(DECLARED, ONE, 0).expect_err("no sample distinguishes it"),
        EvidenceError::EffectIsZero
    );
}

#[test]
fn an_absent_confidence_is_unknowable_rather_than_defaulted() {
    assert_eq!(
        assess(1_000, 0, ONE, ONE),
        Evidence::Unknowable { cause: EvidenceError::NotPositive },
        "the platform chose a confidence level on a Client's behalf"
    );
}

#[test]
fn a_negative_claimed_effect_needs_the_same_evidence_as_its_mirror() {
    assert_eq!(
        observations_required(DECLARED, 2 * ONE, -ONE),
        observations_required(DECLARED, 2 * ONE, ONE)
    );
}

#[test]
fn an_underpowered_run_is_short_by_a_stated_amount() {
    assert_eq!(
        assess(10, DECLARED, 2 * ONE, ONE),
        Evidence::Insufficient { required: 16, observed: 10, short_by: 6 }
    );
}

#[test]
fn evidence_that_cannot_be_sized_does_not_support_a_promotion() {
    assert!(!Evidence::Unknowable { cause: EvidenceError::Overflow }.supports_promotion());
    assert!(
        !Evidence::Insufficient { required: 16, observed: 1, short_by: 15 }.supports_promotion()
    );
    assert!(Evidence::Sufficient { required: 4, observed: 4 }.supports_promotion());
}

fn held_out() -> WalkForward {
    WalkForward::declare(vec![fold((0, 1_000), (1_000, 1_500))], Window::ToPosition(1_500))
        .expect("valid")
}

#[test]
fn a_held_out_result_with_the_evidence_behind_it_is_promotable() {
    let s = stand(Some(&held_out()), assess(100, DECLARED, 2 * ONE, ONE));
    assert!(s.may_promote(), "{s}");
    assert!(matches!(s, Standing::Promotable { held_out_positions: 500, .. }), "{s:?}");
}

#[test]
fn a_result_measured_only_where_it_was_fitted_is_labelled_and_not_promotable() {
    let s = stand(None, assess(10_000, DECLARED, 2 * ONE, ONE));
    assert!(!s.may_promote());
    assert!(
        matches!(s, Standing::InSampleOnly { .. }),
        "an in-sample result with plenty of observations was promotable: {s:?}"
    );
    // Carried, not discarded: the comparison is the finding.
    assert!(format!("{s}").contains("sufficient"), "the evidence was dropped: {s}");
}

#[test]
fn a_held_out_result_nobody_had_the_observations_to_read_is_not_promotable() {
    let s = stand(Some(&held_out()), assess(3, DECLARED, 2 * ONE, ONE));
    assert!(!s.may_promote());
    assert!(matches!(s, Standing::Underpowered { .. }), "{s:?}");
}

use bijective_strategy_engine::runs::Research;

fn spec(bound: u64) -> bijective_strategy_engine::runs::RunSpec<Research> {
    bijective_strategy_engine::runs::RunSpec::<Research>::declare(
        "carry-v3",
        vec!["--instrument".into(), "btc-perp".into()],
        std::collections::BTreeMap::new(),
        Some(bound),
        bijective_strategy_engine::runs::Exchange::Stepwise,
    )
    .expect("a research run with a declared position")
}

fn rule(measure: &str, direction: Direction, threshold: i128) -> DecisionRule {
    DecisionRule { measure: measure.to_string(), direction, threshold }
}

fn registered(bound: u64, r: DecisionRule) -> PreRegistration {
    PreRegistration::declare(
        "carry earns above its funding cost",
        r,
        Horizon::After { positions: 60 },
        Vec::new(),
        &spec(bound),
    )
    .expect("a hypothesis and a measure")
}

fn seen(pairs: &[(&str, i128)]) -> Vec<Measured> {
    pairs.iter().map(|(m, v)| Measured { measure: (*m).to_string(), value: *v }).collect()
}

fn enough() -> Evidence {
    assess(1_000, DECLARED, ONE, ONE)
}

#[test]
fn a_figure_meeting_the_declared_rule_is_reported_against_what_was_declared() {
    let reg = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    let v = judge(Some(&reg), spec(4_102).identity(), &seen(&[("net_bps", 40)]), enough());

    assert_eq!(v.outcome, Outcome::Met { observed: 40 });
    assert!(v.outcome.supports_promotion());
}

#[test]
fn a_figure_missing_the_rule_is_reported_and_is_not_a_promotion() {
    let reg = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    let v = judge(Some(&reg), spec(4_102).identity(), &seen(&[("net_bps", 12)]), enough());

    assert_eq!(v.outcome, Outcome::NotMet { observed: 12 });
    assert!(!v.outcome.supports_promotion());
}

/// The direction is half the rule. Without it, a floor and a ceiling are the same declaration.
#[test]
fn a_ceiling_and_a_floor_are_not_the_same_rule() {
    let floor = registered(4_102, rule("drawdown", Direction::AtLeast, 500));
    let ceiling = registered(4_102, rule("drawdown", Direction::AtMost, 500));
    let observed = seen(&[("drawdown", 900)]);
    let id = spec(4_102).identity();

    assert_eq!(
        judge(Some(&floor), id, &observed, enough()).outcome,
        Outcome::Met { observed: 900 }
    );
    assert_eq!(
        judge(Some(&ceiling), id, &observed, enough()).outcome,
        Outcome::NotMet { observed: 900 },
        "a drawdown above its ceiling was reported as meeting it"
    );
}

#[test]
fn a_run_that_never_produced_the_declared_measure_is_unmeasured_not_failed() {
    let reg = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    let v = judge(Some(&reg), spec(4_102).identity(), &seen(&[("gross_bps", 90)]), enough());

    assert_eq!(v.outcome, Outcome::Unmeasured { measure: "net_bps".into() });
    assert!(!v.outcome.supports_promotion());
}

/// The window is in the identity, so a registration cannot move to another stretch of history.
#[test]
fn a_registration_does_not_carry_to_a_run_over_a_different_window() {
    let reg = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    let elsewhere = spec(9_000).identity();
    let v = judge(Some(&reg), elsewhere, &seen(&[("net_bps", 40)]), enough());

    assert!(
        matches!(v.outcome, Outcome::WrongRun { .. }),
        "a claim registered for one window was answered by another: {:?}",
        v.outcome
    );
    assert!(!v.outcome.supports_promotion());
}

#[test]
fn a_verdict_with_no_registration_is_unregistered_and_is_neither_answer() {
    let v = judge(None, spec(4_102).identity(), &seen(&[("net_bps", 40)]), enough());

    assert_eq!(v.outcome, Outcome::Unregistered);
    assert!(!v.outcome.supports_promotion());
    assert_eq!(v.unregistered.len(), 1, "what was looked at was dropped");
}

#[test]
fn figures_nobody_declared_are_carried_apart_from_the_verdict() {
    let reg = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    let v = judge(
        Some(&reg),
        spec(4_102).identity(),
        &seen(&[("net_bps", 40), ("sharpe", 21_000), ("hit_rate", 5_600)]),
        enough(),
    );

    assert_eq!(v.outcome, Outcome::Met { observed: 40 });
    assert_eq!(v.unregistered.len(), 2);
    assert!(
        v.unregistered.iter().all(|u| u.measure != "net_bps"),
        "the declared measure was also reported as unregistered"
    );
}

#[test]
fn a_registration_with_no_measure_cannot_be_declared() {
    assert_eq!(
        PreRegistration::declare(
            "something happens",
            rule("", Direction::AtLeast, 1),
            Horizon::NotHeld,
            Vec::new(),
            &spec(4_102)
        )
        .expect_err("a rule any figure would meet"),
        RegistrationError::NoMeasure
    );
}

#[test]
fn a_registration_with_no_hypothesis_cannot_be_declared() {
    assert_eq!(
        PreRegistration::declare(
            "   ",
            rule("net_bps", Direction::AtLeast, 25),
            Horizon::NotHeld,
            Vec::new(),
            &spec(4_102)
        )
        .expect_err("nothing claimed"),
        RegistrationError::NoHypothesis
    );
}

#[test]
fn changing_the_run_changes_the_identity_a_registration_binds_to() {
    let first = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    let amended = registered(4_102, rule("net_bps", Direction::AtLeast, 40));

    assert_eq!(first.run(), amended.run(), "the same run, a corrected threshold");
    assert_ne!(
        first.run(),
        registered(9_000, rule("net_bps", Direction::AtLeast, 25)).run(),
        "a different window produced the same identity"
    );
}

use crate::evidence::registry;
use bijective_event_store::store::StreamWriter;

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn root(name: &str) -> std::path::PathBuf {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("bijective-evidence-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a place to write");
    dir
}

fn write(dir: &std::path::Path, reg: &PreRegistration, at_ns: i128) {
    let mut w = StreamWriter::open(dir, "registrations").expect("writer");
    w.append(&registry::encode(reg, at_ns).expect("encode")).expect("append");
}

#[test]
fn a_registration_round_trips_and_encodes_the_same_way_twice() {
    for r in [
        registered(4_102, rule("net_bps", Direction::AtLeast, 25)),
        registered(9_000, rule("drawdown", Direction::AtMost, 1_500)),
    ] {
        let first = registry::encode(&r, 77).expect("encode");
        let second = registry::encode(&r, 77).expect("encode");
        assert_eq!(first, second, "encoding must not vary between calls");

        let back = registry::decode(&first).expect("decode");
        assert_eq!(back.run, r.run());
        assert_eq!(&back.rule, r.rule());
        assert_eq!(back.hypothesis, r.hypothesis());
        assert_eq!(back.at_ns, 77);
    }
}

/// A floor and a ceiling differ by one byte on the wire, and reading it wrongly inverts the claim.
#[test]
fn the_direction_survives_the_round_trip() {
    let floor = registered(4_102, rule("drawdown", Direction::AtLeast, 500));
    let ceiling = registered(4_102, rule("drawdown", Direction::AtMost, 500));

    assert_eq!(
        registry::decode(&registry::encode(&floor, 1).unwrap()).unwrap().rule.direction,
        Direction::AtLeast
    );
    assert_eq!(
        registry::decode(&registry::encode(&ceiling, 1).unwrap()).unwrap().rule.direction,
        Direction::AtMost
    );
}

/// Only the class check can refuse this: the body is a valid registration and only the envelope class differs.
#[test]
fn a_payload_from_another_domain_is_refused_rather_than_misread() {
    let valid = registry::encode(&registered(4_102, rule("net_bps", Direction::AtLeast, 25)), 1)
        .expect("encode");
    assert!(registry::decode(&valid).is_ok(), "the body itself must be readable");

    let mut w = bijective_serialization::codec::Writer::new(
        bijective_serialization::codec::ClassId::OrderIntent,
        1,
    );
    w.bytes(&registered(4_102, rule("net_bps", Direction::AtLeast, 25)).run().0).expect("bytes");
    w.str("carry earns above its funding cost").expect("str");
    w.str("net_bps").expect("str");
    w.u8(0);
    w.i128(25);
    w.i128(1);

    assert!(
        registry::decode(&w.finish()).is_err(),
        "a readable body under another domain's class decoded as a claim"
    );
}

#[test]
fn the_registration_that_stands_is_the_one_a_verdict_is_judged_against() {
    let dir = root("standing");
    let reg = registered(4_102, rule("net_bps", Direction::AtLeast, 25));
    write(&dir, &reg, 100);

    let standing = registry::standing(&dir, "registrations", spec(4_102).identity())
        .expect("read")
        .expect("a claim was written for this run");
    assert_eq!(standing.rule.threshold, 25);
}

#[test]
fn an_amended_registration_supersedes_without_removing_the_first() {
    let dir = root("amended");
    write(&dir, &registered(4_102, rule("net_bps", Direction::AtLeast, 25)), 100);
    write(&dir, &registered(4_102, rule("net_bps", Direction::AtLeast, 40)), 200);

    let standing = registry::standing(&dir, "registrations", spec(4_102).identity())
        .expect("read")
        .expect("a claim stands");
    assert_eq!(standing.rule.threshold, 40, "the amendment did not supersede");
    assert_eq!(standing.at_ns, 200);

    let all = bijective_event_store::store::read_range(&dir, "registrations", 0, u64::MAX)
        .expect("range");
    assert_eq!(all.len(), 2, "an entry was removed");
    assert_eq!(registry::decode(&all[0].payload).unwrap().rule.threshold, 25);
}

#[test]
fn a_claim_registered_for_one_run_is_not_returned_for_another() {
    let dir = root("other-run");
    write(&dir, &registered(4_102, rule("net_bps", Direction::AtLeast, 25)), 100);

    assert_eq!(
        registry::standing(&dir, "registrations", spec(9_000).identity()).expect("read"),
        None,
        "a claim carried across specifications"
    );
}

#[test]
fn a_verdict_is_judged_against_the_claim_as_it_was_written() {
    let dir = root("end-to-end");
    write(&dir, &registered(4_102, rule("net_bps", Direction::AtLeast, 25)), 100);
    write(&dir, &registered(4_102, rule("net_bps", Direction::AtLeast, 40)), 200);

    let id = spec(4_102).identity();
    let stood = registry::standing(&dir, "registrations", id).expect("read").expect("a claim");
    let reg = PreRegistration::declare(
        stood.hypothesis,
        stood.rule,
        stood.horizon,
        stood.folds.clone(),
        &spec(4_102),
    )
    .expect("valid");

    // 30 met the original threshold and does not meet the amended one.
    assert_eq!(
        judge(Some(&reg), id, &seen(&[("net_bps", 30)]), enough()).outcome,
        Outcome::NotMet { observed: 30 },
        "the verdict was judged against the threshold that was replaced"
    );
}

/// The last position a run may read is testable: a window is inclusive and a fold is half-open.
#[test]
fn a_fold_may_test_the_last_position_the_run_was_allowed_to_read() {
    let folds = vec![Fold { learn: Positions::new(0, 3), test: Positions::new(3, 4) }];

    let held = WalkForward::declare(folds, Window::ToPosition(3));

    assert!(
        held.is_ok(),
        "position 3 is readable, so a test over [3, 4) is inside the window: {held:?}"
    );
}

#[test]
fn a_fold_reaching_beyond_the_window_is_still_refused() {
    let folds = vec![Fold { learn: Positions::new(0, 3), test: Positions::new(3, 5) }];

    match WalkForward::declare(folds, Window::ToPosition(3)) {
        Err(WalkForwardError::OutsideWindow { reaches, window_ends, .. }) => {
            assert_eq!(reaches, 5);
            assert_eq!(window_ends, 3);
        }
        other => panic!("a test over history the run never read must be refused: {other:?}"),
    }
}

#[test]
fn a_horizon_of_no_positions_is_refused() {
    let declared = PreRegistration::declare(
        "it earns something over no time at all",
        rule("net_bps", Direction::AtLeast, 25),
        Horizon::After { positions: 0 },
        Vec::new(),
        &spec(4_102),
    );

    assert_eq!(declared, Err(RegistrationError::HorizonOfNothing));
}
