// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

const BEAT_NS: i128 = 5_000_000_000;

fn market(times_ns: &[i128]) -> Vec<MarketEntry> {
    times_ns
        .iter()
        .enumerate()
        .map(|(i, at_ns)| MarketEntry { position: i as u64, at_ns: *at_ns })
        .collect()
}

fn decided(positions: &[u64]) -> Vec<Decided> {
    positions
        .iter()
        .enumerate()
        .map(|(i, at_position)| Decided { seq: i as u64, at_position: *at_position })
        .collect()
}

fn positions(schedule: &Schedule) -> Vec<u64> {
    schedule.steps.iter().map(|s| s.position).collect()
}

#[test]
fn a_pass_runs_at_the_first_entry_each_decision_each_heartbeat_and_the_last_entry() {
    let times = [0, 1_000, 2_000, BEAT_NS + 1_000, BEAT_NS + 1_001, BEAT_NS + 1_002];
    let schedule = schedule(&market(&times), &decided(&[1]), BEAT_NS);

    assert_eq!(positions(&schedule), vec![0, 1, 3, 5]);
    assert_eq!(schedule.unreached, 0);
}

#[test]
fn proposals_are_read_only_through_the_last_one_decided_by_the_step() {
    let times = [0, 10, 20, 30];
    let schedule = schedule(&market(&times), &decided(&[1, 1, 3]), BEAT_NS);

    let through: Vec<Option<u64>> = schedule.steps.iter().map(|s| s.proposals_through).collect();
    assert_eq!(positions(&schedule), vec![0, 1, 3]);
    assert_eq!(through, vec![None, Some(1), Some(2)]);
}

/// A later proposal decided at an earlier position waits for the one ahead of it: judging it first
/// would judge it against a limit state its predecessor had not yet changed.
#[test]
fn a_proposal_never_overtakes_the_one_before_it() {
    let schedule = schedule(&market(&[0, 10, 20]), &decided(&[2, 1]), BEAT_NS);

    let last = schedule.steps.last().copied();
    assert_eq!(positions(&schedule), vec![0, 2]);
    assert_eq!(last.and_then(|s| s.proposals_through), Some(1));
}

#[test]
fn the_clock_never_moves_back() {
    let schedule = schedule(&market(&[100, 50, 200]), &decided(&[1]), BEAT_NS);

    let times: Vec<i128> = schedule.steps.iter().map(|s| s.at_ns).collect();
    assert_eq!(times, vec![100, 100, 200]);
}

#[test]
fn a_proposal_decided_past_the_window_is_counted_not_judged() {
    let schedule = schedule(&market(&[0, 10]), &decided(&[1, 7]), BEAT_NS);

    assert_eq!(schedule.unreached, 1);
    assert_eq!(schedule.steps.last().and_then(|s| s.proposals_through), Some(0));
}

#[test]
fn an_empty_recording_has_no_pass() {
    let schedule = schedule(&[], &decided(&[0]), BEAT_NS);

    assert!(schedule.steps.is_empty());
    assert_eq!(schedule.unreached, 1);
}
