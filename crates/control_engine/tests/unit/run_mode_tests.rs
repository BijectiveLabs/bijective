// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{Inputs, PlanError, RunMode, RunPlan};

const START_NS: i128 = 1_788_000_000_000_000_000;

fn plan(mode: &str, replay_from: Option<i128>) -> RunPlan {
    RunPlan::select(Some(mode), replay_from).expect("a valid plan")
}

/// The default used to be live, which reaches venues. A run that names no mode starts nothing.
#[test]
fn a_run_that_names_no_mode_is_refused() {
    assert_eq!(RunPlan::select(None, None), Err(PlanError::ModeMissing));
    assert_eq!(RunPlan::select(None, Some(START_NS)), Err(PlanError::ModeMissing));
}

/// Replay is how a run is fed, not what it is for, and shadow was renamed to paper.
#[test]
fn replay_and_shadow_are_not_modes() {
    for name in ["replay", "shadow", "Live", ""] {
        assert_eq!(
            RunPlan::select(Some(name), None),
            Err(PlanError::UnknownMode(name.to_string())),
            "{name:?}"
        );
    }
}

#[test]
fn a_live_run_reads_the_wall_clock_and_reaches_its_venues() {
    let live = plan("live", None);

    assert_eq!(live.mode(), RunMode::Live);
    assert_eq!(live.inputs(), Inputs::Live);
    assert!(live.reaches_venues());
    assert_eq!(live.name(), "live");
}

#[test]
fn a_paper_run_reads_live_inputs_and_reaches_no_venue() {
    let paper = plan("paper", None);

    assert_eq!(paper.mode(), RunMode::Paper);
    assert_eq!(paper.inputs(), Inputs::Live);
    assert!(!paper.reaches_venues(), "paper must never send an order");
    assert_eq!(paper.name(), "paper");
}

/// A backtest is a run over recorded history: the clock is the recorded sequence and its orders
/// meet simulated venues only (decision 0015).
#[test]
fn a_backtest_runs_on_recorded_inputs_and_a_sequence_clock() {
    let backtest = plan("backtest", Some(START_NS));

    assert_eq!(backtest.mode(), RunMode::Backtest);
    assert_eq!(backtest.inputs(), Inputs::Recorded { from_ns: START_NS });
    assert!(!backtest.reaches_venues());
    assert_eq!(backtest.name(), "backtest");
    assert_eq!(backtest.clock().now_ns(), START_NS, "the clock starts where the recording does");
}

/// Over live data it would be a paper run. Naming the mode without a start would run on the wall
/// clock and report itself as reproducible.
#[test]
fn a_backtest_without_a_recorded_start_is_refused() {
    assert_eq!(
        RunPlan::select(Some("backtest"), None),
        Err(PlanError::BacktestNeedsRecordedInputs)
    );
}

/// Replaying a paper or live run on its own journal is not built. It is refused, never run on
/// the host clock under a mode that suggests it was replayed.
#[test]
fn a_recorded_start_is_refused_for_paper_and_live() {
    for name in ["paper", "live"] {
        assert_eq!(
            RunPlan::select(Some(name), Some(START_NS)),
            Err(PlanError::ReplayIsOnlyBuiltForBacktest { mode: name.to_string() }),
            "{name}"
        );
    }
}

/// Capital reaches a venue in exactly one case. Checked over every combination the command line
/// can express, so a new mode or input cannot reach one by omission.
#[test]
fn only_a_live_mode_on_live_inputs_reaches_a_venue() {
    for name in ["backtest", "paper", "live"] {
        for replay_from in [None, Some(START_NS)] {
            let Ok(plan) = RunPlan::select(Some(name), replay_from) else { continue };
            let reaches = plan.reaches_venues();
            let expected = plan.mode() == RunMode::Live && plan.inputs() == Inputs::Live;
            assert_eq!(reaches, expected, "{name} with {replay_from:?}");
        }
    }
}

#[test]
fn every_refusal_says_what_to_do() {
    for error in [
        PlanError::ModeMissing,
        PlanError::UnknownMode("replay".into()),
        PlanError::BacktestNeedsRecordedInputs,
        PlanError::ReplayIsOnlyBuiltForBacktest { mode: "live".into() },
    ] {
        let text = error.to_string();
        assert!(text.contains("--mode") || text.contains("--replay-from"), "{text}");
    }
}
