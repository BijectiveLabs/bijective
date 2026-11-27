// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the clock must refuse, and the one property a backtest rests on.

use super::*;

#[test]
fn a_sequence_clock_reads_where_it_was_started() {
    let clock = SequenceClock::starting_at(1_700_000_000_000_000_000);
    assert_eq!(clock.now_ns(), 1_700_000_000_000_000_000);
}

#[test]
fn a_sequence_clock_moves_to_an_events_time() {
    let clock = SequenceClock::starting_at(100);
    clock.advance_to(250).unwrap();
    assert_eq!(clock.now_ns(), 250);
}

/// Two events may share a nanosecond. Refusing that would make the clock reject a real history.
#[test]
fn two_events_at_the_same_instant_are_permitted() {
    let clock = SequenceClock::starting_at(100);
    clock.advance_to(100).unwrap();
    assert_eq!(clock.now_ns(), 100);
}

/// A silent refusal to go back would diverge from the log while reporting success.
#[test]
fn moving_backwards_is_refused_rather_than_ignored() {
    let clock = SequenceClock::starting_at(500);
    assert_eq!(
        clock.advance_to(499).unwrap_err(),
        ClockError::WouldGoBackwards { at_ns: 500, asked_ns: 499 }
    );
    assert_eq!(clock.now_ns(), 500, "and the reading did not move");
}

/// The property the event model rests on: the same sequence read twice reads the same.
#[test]
fn the_same_sequence_produces_identical_readings_twice() {
    let sequence = [10i128, 25, 25, 900, 1_000];

    let run = |seq: &[i128]| {
        let clock = SequenceClock::starting_at(0);
        seq.iter()
            .map(|t| {
                clock.advance_to(*t).unwrap();
                clock.now_ns()
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(run(&sequence), run(&sequence));
}

/// An NTP step backwards must not reorder an ordered log.
#[test]
fn a_wall_clock_never_reads_earlier_than_it_did_before() {
    let clock = WallClock::new();
    let mut last = clock.now_ns();

    for _ in 0..1_000 {
        let now = clock.now_ns();
        assert!(now >= last, "went backwards: {last} then {now}");
        last = now;
    }
}

/// A step back on the host holds the reading still until the host passes it again.
#[test]
fn a_host_step_back_holds_the_reading_still() {
    let last = AtomicI64::new(i64::MIN);
    assert_eq!(later_of(&last, 1_000), 1_000);
    assert_eq!(later_of(&last, 400), 1_000, "a step back moved time backwards");
    assert_eq!(later_of(&last, 999), 1_000);
    assert_eq!(later_of(&last, 1_500), 1_500);
}

/// A suspended host: the monotonic clock stopped, the host clock did not. The next reading is the
/// host's, so a venue request signed after a resume carries the venue's own time, not one lagging
/// by the length of the suspension (measured 2026-11-27: 3923 s, every signed request refused).
#[test]
fn a_reading_after_a_suspension_is_the_hosts_time() {
    let last = AtomicI64::new(i64::MIN);
    let before = 1_795_740_000_000_000_000;
    let suspended_ns = 3_923_000_000_000;
    assert_eq!(later_of(&last, before), before as Nanos);
    assert_eq!(later_of(&last, before + suspended_ns), (before + suspended_ns) as Nanos);
}

/// The host's clock, not an anchor taken at start: two clocks made at different times agree.
#[test]
fn a_wall_clock_follows_the_host() {
    let clock = WallClock::new();
    let host = system_ns() as Nanos;
    let read = clock.now_ns();
    assert!((read - host).abs() < 1_000_000_000, "{read} is not the host's {host}");
}

#[test]
fn a_wall_clock_reads_a_plausible_epoch_time() {
    // A reading of zero is a failed clock that returned a number.
    let now = WallClock::new().now_ns();
    assert!(now > 1_577_836_800_000_000_000, "before 2020: {now}");
    assert!(now < 4_102_444_800_000_000_000, "after 2100: {now}");
}

/// Both are read through the same interface, which is the whole point of there being a trait.
#[test]
fn both_implementations_answer_the_same_question() {
    let clocks: Vec<Box<dyn Clock>> =
        vec![Box::new(WallClock::new()), Box::new(SequenceClock::starting_at(42))];
    for clock in &clocks {
        let _: Nanos = clock.now_ns();
    }
    assert_eq!(clocks[1].now_ns(), 42);
}

/// Dates with independently known answers: epoch, a leap day, and a non-leap century.
#[test]
fn a_reading_renders_as_a_date_a_person_knows() {
    assert_eq!(as_utc(0), "1970-01-01 00:00:00Z");
    assert_eq!(as_utc(1_000_000_000), "1970-01-01 00:00:01Z");
    assert_eq!(as_utc(1_709_164_800_000_000_000), "2024-02-29 00:00:00Z");
    assert_eq!(as_utc(951_782_400_000_000_000), "2000-02-29 00:00:00Z");
    // Before the epoch: the arithmetic floors rather than truncates, so this is not 1969-12-31.
    assert_eq!(as_utc(-1_000_000_000), "1969-12-31 23:59:59Z");
}
