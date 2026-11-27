// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{Fixed, Jitter, SplitMix64};

#[test]
fn the_same_seed_gives_the_same_sequence() {
    let mut a = SplitMix64::seeded(42);
    let mut b = SplitMix64::seeded(42);

    let first: Vec<u64> = (0..50).map(|_| a.below(1_000)).collect();
    let second: Vec<u64> = (0..50).map(|_| b.below(1_000)).collect();

    assert_eq!(first, second);
}

#[test]
fn different_seeds_give_different_sequences() {
    let mut a = SplitMix64::seeded(1);
    let mut b = SplitMix64::seeded(2);

    let first: Vec<u64> = (0..50).map(|_| a.below(1_000_000)).collect();
    let second: Vec<u64> = (0..50).map(|_| b.below(1_000_000)).collect();

    assert_ne!(first, second);
}

#[test]
fn a_draw_is_always_below_its_bound() {
    let mut source = SplitMix64::seeded(7);
    for bound in [1u64, 2, 3, 10, 1_000, u64::MAX / 3 + 1, u64::MAX] {
        for _ in 0..200 {
            assert!(source.below(bound) < bound, "bound {bound}");
        }
    }
}

#[test]
fn a_bound_of_one_has_one_answer() {
    let mut source = SplitMix64::seeded(9);
    assert!((0..20).all(|_| source.below(1) == 0));
}

/// A source that never draws a value outside its range would pass the bounds test while using
/// only part of it. Over many draws every residue of a small bound must appear.
#[test]
fn a_small_bound_is_covered_by_many_draws() {
    let mut source = SplitMix64::seeded(11);
    let mut seen = [false; 8];
    for _ in 0..500 {
        seen[usize::try_from(source.below(8)).expect("fits")] = true;
    }
    assert!(seen.iter().all(|s| *s), "{seen:?}");
}

#[test]
fn a_process_seed_follows_the_clock_it_is_given() {
    let draws = |now_ns| {
        let mut source = SplitMix64::for_this_process(now_ns);
        (0..10).map(|_| source.below(1_000_000)).collect::<Vec<_>>()
    };

    assert_eq!(draws(1_800_000_000_000_000_000), draws(1_800_000_000_000_000_000));
    assert_ne!(draws(1_800_000_000_000_000_000), draws(1_800_000_000_000_000_001));
}

#[test]
fn a_fixed_source_answers_its_value_clamped_to_the_bound() {
    assert_eq!(Fixed(5).below(10), 5);
    assert_eq!(Fixed(50).below(10), 9, "clamped to the largest value below the bound");
    assert_eq!(Fixed(0).below(10), 0);
}
