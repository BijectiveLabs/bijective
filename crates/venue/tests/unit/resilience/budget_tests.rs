// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{Quota, RequestBudget};

/// Ten requests in a second: one every 100 ms, a burst of ten.
const QUOTA: Quota = Quota { requests: 10, per_ns: 1_000_000_000 };
const STEP: i128 = 100_000_000;

fn budget() -> RequestBudget {
    RequestBudget::new(QUOTA).expect("a valid quota")
}

#[test]
fn a_quota_needs_requests_and_a_period() {
    assert!(RequestBudget::new(Quota { requests: 0, per_ns: 1 }).is_err());
    assert!(RequestBudget::new(Quota { requests: 1, per_ns: 0 }).is_err());
    assert!(RequestBudget::new(Quota { requests: 1_000, per_ns: 1 }).is_err(), "no whole interval");
}

#[test]
fn a_full_burst_is_allowed_at_once_and_the_next_request_waits() {
    let mut b = budget();

    for n in 0..10 {
        assert_eq!(b.acquire(0), Ok(()), "request {n}");
    }
    assert_eq!(b.acquire(0), Err(STEP), "the eleventh waits for one interval");
}

#[test]
fn the_wait_shrinks_as_time_passes_and_clears_at_the_interval() {
    let mut b = budget();
    for _ in 0..10 {
        b.acquire(0).expect("burst");
    }

    assert_eq!(b.acquire(STEP / 4), Err(STEP - STEP / 4));
    assert_eq!(b.acquire(STEP), Ok(()));
}

#[test]
fn a_refused_request_does_not_spend_the_budget() {
    let mut b = budget();
    for _ in 0..10 {
        b.acquire(0).expect("burst");
    }
    for _ in 0..50 {
        assert!(b.acquire(0).is_err());
    }

    assert_eq!(b.acquire(STEP), Ok(()), "fifty refusals cost nothing");
}

#[test]
fn an_idle_period_restores_the_burst_but_never_more() {
    let mut b = budget();
    for _ in 0..10 {
        b.acquire(0).expect("burst");
    }

    let later = 100 * 1_000_000_000;
    for n in 0..10 {
        assert_eq!(b.acquire(later), Ok(()), "request {n} after the idle period");
    }
    assert!(b.acquire(later).is_err(), "idle time does not bank more than one burst");
}

/// Over any long run the rate cannot exceed the quota.
#[test]
fn the_long_run_rate_is_the_quota() {
    let mut b = budget();
    let mut granted = 0u32;
    let mut now = 0i128;
    while now < 10 * 1_000_000_000 {
        if b.acquire(now).is_ok() {
            granted += 1;
        }
        now += 1_000_000;
    }
    // Ten seconds at ten per second, plus the initial burst.
    assert!(granted <= 100 + 10, "granted {granted}");
    assert!(granted >= 100, "granted {granted}: the budget must not under-serve");
}

/// The venue's own statement of when to come back outranks the local quota.
#[test]
fn a_retry_after_blocks_until_it_has_passed_even_with_budget_left() {
    let mut b = budget();

    b.retry_after(1_000, 5_000_000_000);

    assert_eq!(b.acquire(1_000), Err(5_000_000_000));
    assert_eq!(b.acquire(1_000 + 4_999_999_999), Err(1));
    assert_eq!(b.acquire(1_000 + 5_000_000_000), Ok(()));
}

#[test]
fn a_shorter_retry_after_never_shortens_a_longer_one() {
    let mut b = budget();

    b.retry_after(0, 5_000);
    b.retry_after(0, 1_000);

    assert_eq!(b.acquire(0), Err(5_000));
}

#[test]
fn a_retry_after_that_is_not_positive_blocks_nothing() {
    let mut b = budget();

    b.retry_after(0, 0);
    b.retry_after(0, -50);

    assert_eq!(b.acquire(0), Ok(()));
}
