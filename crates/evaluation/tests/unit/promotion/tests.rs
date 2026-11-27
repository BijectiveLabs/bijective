// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use crate::evidence::{
    DecisionRule, Direction, Fold, Horizon, Measured, Positions, PreRegistration, assess as size,
    judge,
};
use bijective_strategy_engine::runs::{Research, RunSpec, Window, ledger::ExitReason};

const DECLARED: i128 = 19_600;
const ONE: i128 = 10_000;

fn spec(bound: u64) -> RunSpec<Research> {
    named("carry-v3", bound)
}

fn named(program: &str, bound: u64) -> RunSpec<Research> {
    RunSpec::<Research>::declare(
        program,
        Vec::new(),
        std::collections::BTreeMap::new(),
        Some(bound),
        bijective_strategy_engine::runs::Exchange::Stepwise,
    )
    .expect("a research run with a declared position")
}

fn entry(bound: u64, exit: ExitReason) -> RunLedgerEntry {
    RunLedgerEntry {
        specification: spec(bound).identity(),
        class: "research".into(),
        started_ns: 10,
        ended_ns: 20,
        exit,
        emitted: Digest([7u8; 32]),
        strategy: Some(spec(bound).strategy()),
        program: None,
        content: Some(Digest([9u8; 32])),
    }
}

fn folds() -> Vec<Fold> {
    vec![Fold { learn: Positions::new(0, 1_000), test: Positions::new(1_000, 1_500) }]
}

fn held_out() -> WalkForward {
    WalkForward::declare(
        vec![Fold { learn: Positions::new(0, 1_000), test: Positions::new(1_000, 1_500) }],
        Window::ToPosition(1_500),
    )
    .expect("ordered and inside the window")
}

fn a_good_verdict(bound: u64) -> Verdict {
    let reg = PreRegistration::declare(
        "carry earns above its funding cost",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 25 },
        Horizon::After { positions: 60 },
        folds(),
        &spec(bound),
    )
    .expect("a hypothesis and a measure");

    judge(
        Some(&reg),
        spec(bound).identity(),
        &[Measured { measure: "net_bps".into(), value: 40 }],
        size(1_000, DECLARED, ONE, ONE),
    )
}

fn presented<'a>(
    entry: &'a RunLedgerEntry,
    verdict: &'a Verdict,
    held: Option<&'a WalkForward>,
) -> Presented<'a> {
    Presented {
        entry,
        reproduction: Reproduction::Identical,
        verdict: Some(verdict),
        held_out: held,
    }
}

#[test]
fn a_run_that_reproduced_and_met_its_claim_on_held_out_evidence_is_promoted() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let v = a_good_verdict(4_102);
    let w = held_out();

    let promoted = assess(&presented(&e, &v, Some(&w))).expect("every term is yes");
    assert_eq!(promoted.run, spec(4_102).identity());
    assert_eq!(promoted.held_out_positions, 500);
}

#[test]
fn a_run_that_diverged_on_replay_is_refused_whatever_its_result() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let v = a_good_verdict(4_102);
    let w = held_out();
    let mut p = presented(&e, &v, Some(&w));
    p.reproduction = Reproduction::Diverged { first: Digest([1u8; 32]), again: Digest([2u8; 32]) };

    assert!(matches!(
        assess(&p).expect_err("a divergent run was promoted"),
        Refusal::DidNotReproduce { .. }
    ));
}

#[test]
fn a_run_that_was_never_replayed_is_refused_apart_from_one_that_diverged() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let v = a_good_verdict(4_102);
    let w = held_out();
    let mut p = presented(&e, &v, Some(&w));
    p.reproduction = Reproduction::NotCompared;

    assert_eq!(
        assess(&p).expect_err("an unrepeated run was promoted"),
        Refusal::NotReproduced,
        "an unrepeated run was reported as a divergent one"
    );
}

#[test]
fn a_run_cut_short_cannot_be_promoted_on_what_it_had_reached() {
    for exit in [ExitReason::Timeout, ExitReason::LimitExceeded, ExitReason::Refused] {
        let e = entry(4_102, exit);
        let v = a_good_verdict(4_102);
        let w = held_out();

        assert!(
            matches!(
                assess(&presented(&e, &v, Some(&w))).expect_err("a partial run was promoted"),
                Refusal::OutputUnusable { .. }
            ),
            "{exit:?} was treated as a result"
        );
    }
}

#[test]
fn a_claim_that_was_not_met_is_refused_and_says_what_the_verdict_was() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let reg = PreRegistration::declare(
        "carry earns above its funding cost",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 25 },
        Horizon::After { positions: 60 },
        folds(),
        &spec(4_102),
    )
    .expect("valid");
    let v = judge(
        Some(&reg),
        spec(4_102).identity(),
        &[Measured { measure: "net_bps".into(), value: 12 }],
        size(1_000, DECLARED, ONE, ONE),
    );
    let w = held_out();

    let refused = assess(&presented(&e, &v, Some(&w))).expect_err("a missed claim was promoted");
    assert!(matches!(refused, Refusal::ClaimNotMet { .. }), "{refused:?}");
}

#[test]
fn a_result_measured_only_where_it_was_fitted_is_refused() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let v = a_good_verdict(4_102);

    assert_eq!(
        assess(&presented(&e, &v, None)).expect_err("an in-sample result was promoted"),
        Refusal::InSampleOnly
    );
}

#[test]
fn a_held_out_result_without_the_observations_to_read_it_is_refused() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let reg = PreRegistration::declare(
        "carry earns above its funding cost",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 25 },
        Horizon::After { positions: 60 },
        folds(),
        &spec(4_102),
    )
    .expect("valid");
    let v = judge(
        Some(&reg),
        spec(4_102).identity(),
        &[Measured { measure: "net_bps".into(), value: 40 }],
        size(3, DECLARED, 2 * ONE, ONE),
    );
    let w = held_out();

    let refused =
        assess(&presented(&e, &v, Some(&w))).expect_err("an underpowered result was promoted");
    assert!(matches!(refused, Refusal::EvidenceShort { .. }), "{refused:?}");
}

#[test]
fn a_verdict_with_no_registration_cannot_be_promoted() {
    let e = entry(4_102, ExitReason::Completed { status: 0 });
    let v = judge(
        None,
        spec(4_102).identity(),
        &[Measured { measure: "net_bps".into(), value: 40 }],
        size(1_000, DECLARED, ONE, ONE),
    );
    let w = held_out();

    let refused =
        assess(&presented(&e, &v, Some(&w))).expect_err("an unregistered result was promoted");
    assert!(
        matches!(refused, Refusal::ClaimNotMet { outcome: Outcome::Unregistered }),
        "{refused:?}"
    );
}

#[test]
fn each_refusal_says_which_term_stopped_it() {
    let e = entry(4_102, ExitReason::Timeout);
    let v = a_good_verdict(4_102);
    let w = held_out();

    let said = assess(&presented(&e, &v, Some(&w))).expect_err("refused").to_string();
    assert!(said.contains("cut short"), "the refusal does not name its term: {said}");
}

fn decision(bound: u64) -> Promoted {
    let e = entry(bound, ExitReason::Completed { status: 0 });
    let v = a_good_verdict(bound);
    let w = held_out();
    assess(&presented(&e, &v, Some(&w))).expect("every term is yes")
}

#[test]
fn live_follows_paper_follows_backtest() {
    let back = Cleared::<Backtest>::first(decision(4_102)).expect("a strategy the ledger names");
    let paper = Cleared::<Paper>::after(back, decision(4_102)).expect("the same strategy");
    let live = Cleared::<Live>::after(paper, decision(4_102)).expect("the same strategy");

    assert_eq!(live.stage(), "live");
    assert_eq!(live.run(), spec(4_102).identity());
}

#[test]
fn a_stage_cleared_for_another_strategy_is_not_a_predecessor() {
    let back = Cleared::<Backtest>::first(decision(4_102)).expect("a strategy the ledger names");

    let mut elsewhere = entry(4_102, ExitReason::Completed { status: 0 });
    elsewhere.strategy = Some(named("carry-v4", 4_102).strategy());
    let v = a_good_verdict(4_102);
    let w = held_out();
    let other = assess(&Presented {
        entry: &elsewhere,
        reproduction: Reproduction::Identical,
        verdict: Some(&v),
        held_out: Some(&w),
    })
    .expect("every term is yes");

    let refused = Cleared::<Paper>::after(back, other)
        .expect_err("one strategy's backtest cleared another strategy's paper");
    assert!(matches!(refused, StageError::PriorStageWasAnotherStrategy { .. }), "{refused:?}");
}

/// The same strategy over a different window is a predecessor. Run digests differ by construction.
#[test]
fn the_same_strategy_over_a_different_window_is_a_predecessor() {
    let back = Cleared::<Backtest>::first(decision(4_102)).expect("a strategy the ledger names");

    let paper = Cleared::<Paper>::after(back, decision(9_000));

    assert!(paper.is_ok(), "one strategy, two windows, one path: {paper:?}");
}

#[test]
fn a_decision_about_a_run_with_no_strategy_recorded_clears_nothing() {
    let mut unnamed = entry(4_102, ExitReason::Completed { status: 0 });
    unnamed.strategy = None;
    let v = a_good_verdict(4_102);
    let w = held_out();
    let decision = assess(&Presented {
        entry: &unnamed,
        reproduction: Reproduction::Identical,
        verdict: Some(&v),
        held_out: Some(&w),
    })
    .expect("every term is yes");

    assert!(matches!(
        Cleared::<Backtest>::first(decision),
        Err(StageError::StrategyNotRecorded { .. })
    ));
}

/// Skipping is unsayable: `Cleared::<Live>::after` takes a cleared paper, so no expression asks for live off a backtest.
#[test]
fn the_stage_before_live_is_paper_and_the_one_before_paper_is_backtest() {
    fn requires<S: Stage>() -> &'static str {
        S::NAME
    }
    assert_eq!(requires::<Live>(), "live");
    assert_eq!(requires::<Paper>(), "paper");
    assert_eq!(requires::<Backtest>(), "backtest");

    // The compiler holds the rest. See tests/skipping_a_stage.rs.
}

use crate::evidence::registry::{REGISTRATIONS, declare};
use crate::promotion::path::{
    Gathered, Measurement, NotCleared, clear_backtest, clear_paper, decide, gather,
};
use bijective_event_store::store::StreamWriter;
use bijective_strategy_engine::ledger::{RUNS, record};

fn store(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bijective-promotion-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn ran(dir: &std::path::Path, bound: u64, exit: ExitReason, emitted: Digest) {
    let mut ledger = StreamWriter::open(dir, RUNS).expect("ledger");
    let entry = RunLedgerEntry {
        specification: spec(bound).identity(),
        class: "research".into(),
        started_ns: 10,
        ended_ns: 20,
        exit,
        emitted,
        strategy: Some(spec(bound).strategy()),
        program: None,
        content: None,
    };
    record(&entry, &mut ledger).expect("record");
}

fn measured_at(value: i128) -> Vec<Measured> {
    vec![Measured { measure: "net_bps".into(), value }]
}

#[test]
fn a_run_with_no_entry_is_not_a_decision() {
    let dir = store("absent");

    assert!(gather(&dir, spec(4_102).identity()).expect("gather").is_none());
}

#[test]
fn a_run_that_was_never_replayed_is_refused_before_its_claim_is_reached() {
    let dir = store("once");
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));

    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    assert_eq!(decide(&gathered, &Measurement::NotMeasured), Err(Refusal::NotReproduced));
}

#[test]
fn a_run_that_answered_differently_on_replay_is_refused_with_both_answers() {
    let dir = store("diverged");
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([2u8; 32]));

    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    assert_eq!(
        decide(&gathered, &Measurement::NotMeasured),
        Err(Refusal::DidNotReproduce { first: Digest([1u8; 32]), again: Digest([2u8; 32]) })
    );
}

#[test]
fn a_reproduced_run_nobody_measured_is_refused_as_unmeasured() {
    let dir = store("unmeasured");
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));

    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    assert_eq!(decide(&gathered, &Measurement::NotMeasured), Err(Refusal::NotMeasured));
    assert_eq!(gathered.reproduction, Reproduction::Identical);
}

/// A claim written afterwards cannot be told from none, so an unregistered run is refused.
#[test]
fn a_run_nobody_registered_is_refused_as_unregistered() {
    let dir = store("unregistered");
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));

    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");
    let decided = decide(
        &gathered,
        &Measurement::Measured {
            measured: &measured_at(40),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    );

    assert_eq!(decided, Err(Refusal::ClaimNotMet { outcome: Outcome::Unregistered }));
}

#[test]
fn a_registration_on_the_stream_is_the_claim_the_decision_is_made_against() {
    let dir = store("registered");
    let registration = PreRegistration::declare(
        "carry earns above its funding cost",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 25 },
        Horizon::After { positions: 60 },
        folds(),
        &spec(4_102),
    )
    .expect("a hypothesis and a measure");
    let mut registry = StreamWriter::open(&dir, REGISTRATIONS).expect("registry");
    declare(&registration, 5, &mut registry).expect("declare");
    drop(registry);

    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    // The figure misses the threshold it was registered against, and the refusal says by how much.
    let missed = decide(
        &gathered,
        &Measurement::Measured {
            measured: &measured_at(10),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    );
    assert_eq!(missed, Err(Refusal::ClaimNotMet { outcome: Outcome::NotMet { observed: 10 } }));

    let promoted = decide(
        &gathered,
        &Measurement::Measured {
            measured: &measured_at(40),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    )
    .expect("every term said yes");
    assert_eq!(
        promoted.run,
        spec(4_102).identity(),
        "the decision names the run it was made about"
    );
}

#[test]
fn a_claim_declared_for_another_run_does_not_carry_across() {
    let dir = store("other-run");
    let elsewhere = PreRegistration::declare(
        "a different experiment",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 1 },
        Horizon::After { positions: 60 },
        folds(),
        &spec(9_999),
    )
    .expect("a hypothesis and a measure");
    let mut registry = StreamWriter::open(&dir, REGISTRATIONS).expect("registry");
    declare(&elsewhere, 5, &mut registry).expect("declare");
    drop(registry);

    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    assert!(gathered.registration.is_none(), "a claim does not carry across specifications");
}

#[test]
fn a_cut_short_run_is_recorded_and_refused() {
    let dir = store("cut-short");
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Timeout, Digest([1u8; 32]));

    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    assert_eq!(gathered.entries.len(), 2, "both runs are on the ledger");
    // The latest entry is the one that was cut short, and reproduction needs two usable runs.
    assert_eq!(decide(&gathered, &Measurement::NotMeasured), Err(Refusal::NotReproduced));
}

#[test]
fn what_was_gathered_names_the_run_it_was_gathered_for() {
    let dir = store("names");
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));

    let Gathered { run, entry, .. } =
        gather(&dir, spec(4_102).identity()).expect("gather").expect("entry");

    assert_eq!(run, spec(4_102).identity());
    assert_eq!(entry.specification, run);
}

/// A claim that declared no structure is refused on that, and the caller cannot supply one.
#[test]
fn a_claim_that_declared_no_structure_is_refused_as_in_sample_only() {
    let dir = store("no-structure");
    let registration = PreRegistration::declare(
        "carry earns above its funding cost",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 25 },
        Horizon::After { positions: 60 },
        Vec::new(),
        &spec(4_102),
    )
    .expect("a claim with nothing held out");
    let mut registry = StreamWriter::open(&dir, REGISTRATIONS).expect("registry");
    declare(&registration, 5, &mut registry).expect("declare");
    drop(registry);

    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    let gathered = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");

    let decided = decide(
        &gathered,
        &Measurement::Measured {
            measured: &measured_at(40),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    );

    assert_eq!(decided, Err(Refusal::InSampleOnly));
}

use bijective_strategy_engine::runs::Live as LiveClass;

fn live_spec() -> bijective_strategy_engine::runs::RunSpec<LiveClass> {
    bijective_strategy_engine::runs::RunSpec::<LiveClass>::declare(
        "carry-v3",
        Vec::new(),
        std::collections::BTreeMap::new(),
        None,
        bijective_strategy_engine::runs::Exchange::Stepwise,
    )
    .expect("a live session declares no bound")
}

fn paper_session(dir: &std::path::Path, emitted: Digest) {
    let mut ledger = StreamWriter::open(dir, RUNS).expect("ledger");
    let entry = RunLedgerEntry {
        specification: live_spec().identity(),
        class: "live".into(),
        started_ns: 30,
        ended_ns: 40,
        exit: ExitReason::Completed { status: 0 },
        emitted,
        strategy: Some(live_spec().strategy()),
        program: None,
        content: None,
    };
    record(&entry, &mut ledger).expect("record");
}

fn registered_for(dir: &std::path::Path, run: Digest, folds: Vec<Fold>) {
    let registration = PreRegistration::declare(
        "carry earns above its funding cost",
        DecisionRule { measure: "net_bps".into(), direction: Direction::AtLeast, threshold: 25 },
        Horizon::After { positions: 60 },
        folds,
        &spec(4_102),
    )
    .expect("a claim");
    let mut registry = StreamWriter::open(dir, REGISTRATIONS).expect("registry");
    declare(&registration, 5, &mut registry).expect("declare");
    let _ = run;
}

#[test]
fn a_backtest_clears_and_a_paper_session_of_the_same_strategy_clears_after_it() {
    let dir = store("stages");
    registered_for(&dir, spec(4_102).identity(), folds());
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));

    let backtest = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");
    let cleared = clear_backtest(
        &backtest,
        &Measurement::Measured {
            measured: &measured_at(40),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    )
    .expect("every term is yes");

    assert_eq!(cleared.stage(), "backtest");
    assert_eq!(cleared.strategy(), Some(spec(4_102).strategy()));

    paper_session(&dir, Digest([2u8; 32]));
    paper_session(&dir, Digest([2u8; 32]));
    let session = gather(&dir, live_spec().identity()).expect("gather").expect("an entry");
    assert_ne!(session.run, backtest.run, "each stage is a different run by construction");

    let refused = clear_paper(
        cleared,
        &session,
        &Measurement::Measured {
            measured: &measured_at(40),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    );

    // The claim was declared for the backtest, so the promotion stages joins and the terms stop it.
    assert_eq!(
        refused,
        Err(NotCleared::Terms(Refusal::ClaimNotMet { outcome: Outcome::Unregistered })),
        "the join held; the claim is what was missing"
    );
}

#[test]
fn a_paper_session_of_another_strategy_does_not_follow_this_backtest() {
    let dir = store("stages-other");
    registered_for(&dir, spec(4_102).identity(), folds());
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    ran(&dir, 4_102, ExitReason::Completed { status: 0 }, Digest([1u8; 32]));
    let backtest = gather(&dir, spec(4_102).identity()).expect("gather").expect("an entry");
    let cleared = clear_backtest(
        &backtest,
        &Measurement::Measured {
            measured: &measured_at(40),
            evidence: size(1_000, DECLARED, ONE, ONE),
        },
    )
    .expect("every term is yes");

    let mut elsewhere = entry(4_102, ExitReason::Completed { status: 0 });
    elsewhere.strategy = Some(named("carry-v4", 4_102).strategy());
    let v = a_good_verdict(4_102);
    let w = held_out();
    let other = assess(&Presented {
        entry: &elsewhere,
        reproduction: Reproduction::Identical,
        verdict: Some(&v),
        held_out: Some(&w),
    })
    .expect("every term is yes");

    assert!(matches!(
        Cleared::<Paper>::after(cleared, other),
        Err(StageError::PriorStageWasAnotherStrategy { .. })
    ));
}

/// A clearance does not cross a change of bytes. Reproduction cannot catch it.
#[test]
fn a_stage_cleared_for_one_version_does_not_clear_another() {
    use crate::promotion::stage::{Backtest, Cleared, Paper, StageError};
    use bijective_event_store::digest::sha256;

    let strategy = sha256(b"alpha");
    let cleared = Cleared::<Backtest>::first(Promoted {
        run: sha256(b"backtest"),
        strategy: Some(strategy),
        content: Some(sha256(b"version one")),
        held_out_positions: 4,
    })
    .expect("a backtest with a strategy and a version");

    let same = Promoted {
        run: sha256(b"paper"),
        strategy: Some(strategy),
        content: Some(sha256(b"version one")),
        held_out_positions: 6,
    };
    assert!(
        Cleared::<Paper>::after(cleared, same).is_ok(),
        "one version travels the promotion stages"
    );

    let other = Promoted { content: Some(sha256(b"version two")), ..same };
    match Cleared::<Paper>::after(cleared, other) {
        Err(StageError::PriorStageRanOtherBytes { .. }) => {}
        other => panic!("a clearance crossed a change of bytes: {other:?}"),
    }

    // A run the ledger cannot name by version is refused, like an unrecorded strategy.
    let unrecorded = Promoted { content: None, ..same };
    match Cleared::<Paper>::after(cleared, unrecorded) {
        Err(StageError::BytesNotRecorded { .. }) => {}
        other => panic!("a run with no recorded version was carried: {other:?}"),
    }
}
