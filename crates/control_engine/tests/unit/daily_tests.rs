// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The day's baseline as the control stream records it.

use bijective_event_store::projection::Projection;

use super::Daily;
use crate::wire::ControlEvent;

fn opened(day: u64, unrealized_scaled: i128) -> ControlEvent {
    ControlEvent::DayOpened { day, unrealized_scaled, at_ns: 1 }
}

#[test]
fn no_baseline_is_held_until_a_day_has_opened() {
    let daily = Daily::default();
    assert_eq!(daily.baseline_for(20_000), None);
}

#[test]
fn a_baseline_answers_only_for_its_own_day() {
    let mut daily = Daily::default();
    daily.apply(&opened(20_000, -500)).expect("applied");
    assert_eq!(daily.baseline_for(20_000), Some(-500));
    assert_eq!(daily.baseline_for(20_001), None, "yesterday's baseline is no baseline today");
}

#[test]
fn the_first_baseline_of_a_day_stands() {
    let mut daily = Daily::default();
    daily.apply(&opened(20_000, 100)).expect("applied");
    daily.apply(&opened(20_000, 900)).expect("applied");
    assert_eq!(daily.baseline_for(20_000), Some(100), "a later figure would hide the loss so far");
    daily.apply(&opened(20_001, 7)).expect("applied");
    assert_eq!(daily.baseline_for(20_001), Some(7));
}

fn peaked(day: u64, result_scaled: i128) -> ControlEvent {
    ControlEvent::DayPeak { day, result_scaled, at_ns: 2 }
}

/// A restart must measure from the highest point already reached, not from the open.
#[test]
fn the_peak_only_rises_within_its_day_and_a_new_day_starts_its_own() {
    let mut daily = Daily::default();
    assert_eq!(daily.peak_for(20_000), None);
    daily.apply(&peaked(20_000, 300)).expect("applied");
    daily.apply(&peaked(20_000, 100)).expect("applied");
    assert_eq!(daily.peak_for(20_000), Some(300), "a lower record lowered the peak");
    daily.apply(&peaked(20_001, -5)).expect("applied");
    assert_eq!(daily.peak_for(20_001), Some(-5));
    assert_eq!(daily.peak_for(20_000), None, "yesterday's peak is no peak today");
    daily.apply(&peaked(20_000, 900)).expect("applied");
    assert_eq!(daily.peak_for(20_001), Some(-5), "an older day's record changed today");
}
