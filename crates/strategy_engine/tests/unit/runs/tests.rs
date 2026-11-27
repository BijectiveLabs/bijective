// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The run specification's gates.

use super::*;

fn env() -> BTreeMap<String, String> {
    let mut e = BTreeMap::new();
    e.insert("PATH".to_string(), "/usr/bin".to_string());
    e
}

/// Refused, not widened to the present: widening would be invisible and the verdict would be leakage.
#[test]
fn a_research_run_without_a_position_is_refused() {
    let outcome = RunSpec::<Research>::declare("model", vec![], env(), None, Exchange::Stepwise);

    assert_eq!(outcome, Err(SpecError::NoWindow));
}

#[test]
fn a_research_runs_window_stops_where_it_was_declared() {
    let spec =
        RunSpec::<Research>::declare("model", vec![], env(), Some(4_102), Exchange::Stepwise)
            .expect("declare");

    assert_eq!(spec.window(), Window::ToPosition(4_102));
    assert_eq!(spec.class(), "research");
}

/// An observer's bound is not refused but not consulted: the window belongs to the class.
#[test]
fn an_observer_reads_to_the_present_regardless_of_what_it_was_offered() {
    let spec = RunSpec::<Observer>::declare("watch", vec![], env(), Some(10), Exchange::Stepwise)
        .expect("declare");

    assert_eq!(spec.window(), Window::ToPresent);
}

#[test]
fn an_auditor_sees_the_record_and_no_market_data() {
    let spec = RunSpec::<Auditor>::declare("audit", vec![], env(), None, Exchange::Stepwise)
        .expect("declare");

    assert_eq!(spec.window(), Window::RecordOnly);
}

#[test]
fn a_run_with_nothing_to_execute_is_refused() {
    assert_eq!(
        RunSpec::<Observer>::declare("   ", vec![], env(), None, Exchange::Stepwise),
        Err(SpecError::NoProgram)
    );
}

#[test]
fn identity_follows_the_specification() {
    let a = RunSpec::<Research>::declare(
        "model",
        vec!["--fast".into()],
        env(),
        Some(100),
        Exchange::Stepwise,
    )
    .expect("declare");
    let same = RunSpec::<Research>::declare(
        "model",
        vec!["--fast".into()],
        env(),
        Some(100),
        Exchange::Stepwise,
    )
    .expect("declare");
    let moved = RunSpec::<Research>::declare(
        "model",
        vec!["--fast".into()],
        env(),
        Some(101),
        Exchange::Stepwise,
    )
    .expect("declare");
    let rearmed = RunSpec::<Research>::declare(
        "model",
        vec!["--slow".into()],
        env(),
        Some(100),
        Exchange::Stepwise,
    )
    .expect("declare");

    assert_eq!(a.identity(), same.identity());
    assert_ne!(a.identity(), moved.identity(), "a different window is a different experiment");
    assert_ne!(a.identity(), rearmed.identity());
}

#[test]
fn the_class_is_part_of_the_identity() {
    let research =
        RunSpec::<Research>::declare("model", vec![], env(), Some(0), Exchange::Stepwise)
            .expect("declare");
    let observer = RunSpec::<Observer>::declare("model", vec![], env(), None, Exchange::Stepwise)
        .expect("declare");

    assert_ne!(research.identity(), observer.identity());
}

/// A `Collector<Observer>` taking a `Proposal` is a compile error, the stronger statement. This asserts each takes its own kind.
#[test]
fn each_class_collects_only_its_own_kind() {
    let mut proposals = Collector::<Research>::new();
    proposals.take(Proposal {
        desk: "alpha".into(),
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        side: bijective_model::Side::Buy,
        quantity_scaled: 1_500,
        price_scaled: Some(200),
        cap_scaled: None,
        client_id: "abc123".into(),
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: false,
        parent: None,
        oco: None,
    });

    let mut observations = Collector::<Observer>::new();
    observations
        .take(Observation { subject: "data-engine".into(), detail: "quiet for 40s".into() });

    assert_eq!(proposals.taken().len(), 1);
    assert_eq!(observations.taken().len(), 1);
}

#[test]
fn a_cut_short_run_produces_nothing_usable() {
    assert!(ExitReason::Completed { status: 0 }.output_is_usable());
    assert!(!ExitReason::Completed { status: 1 }.output_is_usable());
    assert!(!ExitReason::Timeout.output_is_usable());
    assert!(!ExitReason::LimitExceeded.output_is_usable());
    assert!(!ExitReason::Refused.output_is_usable());
}

/// The environment is inside the run's identity: runs importing from different places can emit different bytes.
#[test]
fn a_declared_environment_makes_a_different_run() {
    let bare = RunSpec::<Research>::declare(
        "/strategies/one.py".to_string(),
        Vec::new(),
        BTreeMap::new(),
        Some(10),
        Exchange::Stepwise,
    )
    .expect("declare");

    let mut environment = BTreeMap::new();
    environment.insert("PYTHONPATH".to_string(), "/opt/bijective/lib/python".to_string());
    let declared = RunSpec::<Research>::declare(
        "/strategies/one.py".to_string(),
        Vec::new(),
        environment.clone(),
        Some(10),
        Exchange::Stepwise,
    )
    .expect("declare");

    assert_ne!(
        bare.identity(),
        declared.identity(),
        "reproducing a run means reproducing its environment"
    );
    // The stage sequence does not see it: with PYTHONPATH in the strategy digest, a Python strategy was invisible to `bijective-promote`.
    assert_eq!(
        bare.strategy(),
        declared.strategy(),
        "where a library sits is a fact about the run, not about the strategy"
    );

    let same = RunSpec::<Research>::declare(
        "/strategies/one.py".to_string(),
        Vec::new(),
        environment,
        Some(10),
        Exchange::Stepwise,
    )
    .expect("declare");
    assert_eq!(declared.identity(), same.identity(), "re-running it is the same run");
}

#[test]
fn a_paper_session_writes_where_the_control_engine_does_not_read() {
    assert_eq!(Stage::Live.proposals(), "proposals");
    assert_ne!(Stage::Paper.proposals(), Stage::Live.proposals());
}

/// Neither does a backtest: the Control Engine never asks where an entry came from, so research on its stream would be an instruction.
#[test]
fn a_bounded_research_run_writes_where_the_control_engine_does_not_read() {
    assert_ne!(RESEARCH_PROPOSALS, Stage::Live.proposals());
    assert_ne!(RESEARCH_PROPOSALS, Stage::Paper.proposals());
}

/// An unset stage is refused. A default would route proposals to a venue no configuration selected.
#[test]
fn a_stage_that_was_not_written_is_refused() {
    assert_eq!(Stage::parse("paper"), Some(Stage::Paper));
    assert_eq!(Stage::parse("live"), Some(Stage::Live));
    for other in ["", "Paper", "LIVE", "backtest", "shadow"] {
        assert_eq!(Stage::parse(other), None, "{other:?} is not a stage this build runs");
    }
}

#[test]
fn a_class_that_reads_the_present_claims_no_replay() {
    assert_eq!(replays("research"), Some(true));
    assert_eq!(replays("auditor"), Some(true));
    assert_eq!(replays("live"), Some(false), "two sessions read different markets");
    assert_eq!(replays("observer"), Some(false));
    assert_eq!(replays("something-later"), None, "a class this build cannot name says so");
}

#[test]
fn how_a_run_was_fed_is_part_of_which_run_it_is() {
    let whole = RunSpec::<Research>::declare("model", vec![], env(), Some(4_102), Exchange::Whole)
        .expect("declare");
    let stepwise =
        RunSpec::<Research>::declare("model", vec![], env(), Some(4_102), Exchange::Stepwise)
            .expect("declare");

    assert_ne!(whole.identity(), stepwise.identity(), "two runs");
    assert_eq!(whole.strategy(), stepwise.strategy(), "one strategy");
}

#[test]
fn only_a_stepwise_exchange_places_decisions() {
    assert!(Exchange::Stepwise.places_decisions());
    assert!(!Exchange::Whole.places_decisions());
    assert_eq!(Exchange::parse("stepwise"), Some(Exchange::Stepwise));
    assert_eq!(Exchange::parse("lockstep"), None, "an exchange this build does not run says so");
}

/// The name is carried so a surface can list what an operator has run. An entry from before version 3 decodes as not recorded.
#[test]
fn an_entry_carries_the_program_it_was_of_and_survives_a_round_trip() {
    use crate::runs::ledger::{decode, encode};

    let entry = RunLedgerEntry {
        specification: sha256(b"spec"),
        class: "research".to_string(),
        started_ns: 1_788_000_000_000_000_000,
        ended_ns: 1_788_000_001_000_000_000,
        exit: ExitReason::Completed { status: 0 },
        emitted: sha256(b"out"),
        strategy: Some(sha256(b"strategy")),
        program: Some("/strategies/alpha.py".to_string()),
        content: None,
    };

    let read = decode(&encode(&entry).expect("encode")).expect("decode");
    assert_eq!(read.program.as_deref(), Some("/strategies/alpha.py"));
    assert_eq!(read, entry);

    let unnamed = RunLedgerEntry { program: None, ..entry };
    let read = decode(&encode(&unnamed).expect("encode")).expect("decode");
    assert_eq!(read.program, None, "a name not recorded is not an empty name");
}

/// Which bytes ran is a fact about the run, not the strategy's identity: a stage sequence must not carry a clearance across different bytes.
#[test]
fn an_entry_carries_the_bytes_it_ran_and_absent_is_not_empty() {
    use crate::runs::ledger::{decode, encode};

    let entry = RunLedgerEntry {
        specification: sha256(b"spec"),
        class: "research".to_string(),
        started_ns: 1,
        ended_ns: 2,
        exit: ExitReason::Completed { status: 0 },
        emitted: sha256(b"out"),
        strategy: Some(sha256(b"strategy")),
        program: Some("/strategies/alpha.py".to_string()),
        content: Some(sha256(b"#!/bin/sh\n")),
    };

    let read = decode(&encode(&entry).expect("encode")).expect("decode");
    assert_eq!(read, entry);

    let unrecorded = RunLedgerEntry { content: None, ..entry };
    let read = decode(&encode(&unrecorded).expect("encode")).expect("decode");
    assert_eq!(read.content, None, "bytes not recorded are not bytes that were empty");
}

/// A declared name survives the file moving, a path does not. The two digest spaces cannot collide.
#[test]
fn a_declared_name_is_the_strategy_and_the_path_is_not() {
    let here = RunSpec::<Research>::declare(
        "/strategies/alpha.sh".to_string(),
        Vec::new(),
        BTreeMap::new(),
        Some(10),
        Exchange::Stepwise,
    )
    .expect("declare");
    let moved = RunSpec::<Research>::declare(
        "/elsewhere/renamed.sh".to_string(),
        Vec::new(),
        BTreeMap::new(),
        Some(10),
        Exchange::Stepwise,
    )
    .expect("declare");

    assert_ne!(here.strategy(), moved.strategy(), "unnamed, a move is a different strategy");

    let here = here.called("alpha");
    let moved = moved.called("alpha");
    assert_eq!(here.strategy(), moved.strategy(), "named, a move keeps the history");
    assert_eq!(here.strategy(), RunSpec::<Research>::digest_of_name("alpha"));

    // A strategy called `alpha` and one whose program is the path `alpha` are different strategies.
    let by_path = RunSpec::<Research>::declare(
        "alpha".to_string(),
        Vec::new(),
        BTreeMap::new(),
        Some(10),
        Exchange::Stepwise,
    )
    .expect("declare");
    assert_ne!(here.strategy(), by_path.strategy());

    assert_ne!(here.identity(), moved.identity(), "a different program is a different run");
}
