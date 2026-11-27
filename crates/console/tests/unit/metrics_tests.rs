// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The strip as an exposition: one series per label, at its highest level, and a closing marker.

use super::*;

fn signal(label: &'static str, level: Level) -> Signal {
    Signal { label, value: "any".into(), level }
}

#[test]
fn every_label_is_one_series_at_its_highest_level() {
    let text = render(&[
        signal("trading state", Level::Alarm),
        signal("held proposals", Level::Normal),
        signal("trading state", Level::Attention),
    ]);
    assert!(text.contains("bijective_signal{label=\"trading state\"} 2\n"), "{text}");
    assert!(text.contains("bijective_signal{label=\"held proposals\"} 0\n"), "{text}");
    assert_eq!(text.matches("label=\"trading state\"").count(), 1);
    assert!(text.contains("bijective_alarms 1\n"));
    assert!(text.ends_with("# EOF\n"));
}

#[test]
fn nothing_on_the_strip_is_no_alarm_and_still_a_complete_exposition() {
    let text = render(&[]);
    assert!(text.contains("bijective_alarms 0\n") && text.ends_with("# EOF\n"), "{text}");
}

#[test]
fn a_label_value_is_escaped() {
    assert_eq!(escaped("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
}
