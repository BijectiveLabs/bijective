// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use crate::resilience::{Backoff, Fixed, SplitMix64};

const BASE: i128 = 250_000_000;
const CEILING: i128 = 5_000_000_000;

fn backoff() -> Backoff {
    Backoff::new(BASE, CEILING).expect("a valid backoff")
}

#[test]
fn a_backoff_needs_a_positive_base_and_a_ceiling_at_least_the_base() {
    assert!(Backoff::new(0, CEILING).is_err());
    assert!(Backoff::new(-1, CEILING).is_err());
    assert!(Backoff::new(BASE, BASE - 1).is_err());
    assert!(Backoff::new(BASE, BASE).is_ok());
}

#[test]
fn the_longest_wait_doubles_with_each_consecutive_failure_up_to_the_ceiling() {
    let b = backoff();

    assert_eq!(b.longest_ns(0), BASE);
    assert_eq!(b.longest_ns(1), 2 * BASE);
    assert_eq!(b.longest_ns(2), 4 * BASE);
    assert_eq!(b.longest_ns(3), 8 * BASE);
    assert_eq!(b.longest_ns(4), 16 * BASE);
    assert_eq!(b.longest_ns(5), CEILING, "20 bases is past the ceiling");
    assert_eq!(b.longest_ns(6), CEILING);
}

/// A reconnect loop that has run for a day counts failures in the thousands.
#[test]
fn no_number_of_failures_overflows_the_wait() {
    let b = backoff();
    for failures in [63, 64, 100, 127, 128, 1_000, u32::MAX] {
        assert_eq!(b.longest_ns(failures), CEILING, "{failures}");
    }
}

#[test]
fn a_wait_is_between_half_the_longest_and_the_longest() {
    let b = backoff();
    for failures in 0..10 {
        let longest = b.longest_ns(failures);
        for source in 0..20u64 {
            let mut jitter = SplitMix64::seeded(source);
            let wait = b.delay_ns(failures, &mut jitter);
            assert!(wait >= longest / 2 && wait <= longest, "{failures}: {wait} of {longest}");
        }
    }
}

#[test]
fn the_edges_of_the_jitter_are_the_edges_of_the_wait() {
    let b = backoff();

    assert_eq!(b.delay_ns(2, &mut Fixed(0)), 2 * BASE, "the least: half of 4 bases");
    assert_eq!(b.delay_ns(2, &mut Fixed(u64::MAX)), 4 * BASE, "the most: the longest");
}

#[test]
fn a_wait_never_exceeds_the_ceiling() {
    let b = backoff();
    let mut jitter = SplitMix64::seeded(3);
    for failures in 0..200 {
        assert!(b.delay_ns(failures, &mut jitter) <= CEILING);
    }
}

#[test]
fn the_same_seed_gives_the_same_waits() {
    let b = backoff();
    let waits = |seed| {
        let mut jitter = SplitMix64::seeded(seed);
        (0..12).map(|n| b.delay_ns(n, &mut jitter)).collect::<Vec<_>>()
    };

    assert_eq!(waits(5), waits(5));
    assert_ne!(waits(5), waits(6), "two clients must not retry in step");
}
