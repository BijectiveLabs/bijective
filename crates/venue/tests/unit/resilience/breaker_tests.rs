// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{BreakerConfig, CircuitBreaker, CircuitState, Permit};

const OPEN_FOR: i128 = 10_000;

fn breaker() -> CircuitBreaker {
    CircuitBreaker::new(BreakerConfig { failures_to_open: 3, open_for_ns: OPEN_FOR })
        .expect("a valid breaker")
}

fn fail_times(b: &mut CircuitBreaker, times: u32, at: i128) {
    for _ in 0..times {
        assert_eq!(b.permit(at), Permit::Go);
        b.failed(at);
    }
}

#[test]
fn a_breaker_needs_a_threshold_and_an_open_period() {
    assert!(CircuitBreaker::new(BreakerConfig { failures_to_open: 0, open_for_ns: 1 }).is_err());
    assert!(CircuitBreaker::new(BreakerConfig { failures_to_open: 1, open_for_ns: 0 }).is_err());
}

#[test]
fn a_closed_breaker_permits_everything() {
    let mut b = breaker();
    for at in 0..100 {
        assert_eq!(b.permit(at), Permit::Go);
        b.succeeded(at);
    }
    assert_eq!(b.state(), CircuitState::Closed);
}

#[test]
fn consecutive_failures_open_it_at_the_threshold() {
    let mut b = breaker();

    fail_times(&mut b, 2, 100);
    assert_eq!(b.state(), CircuitState::Closed, "two failures are below the threshold");

    assert_eq!(b.permit(100), Permit::Go);
    b.failed(100);
    assert_eq!(b.state(), CircuitState::Open { until_ns: 100 + OPEN_FOR });
}

#[test]
fn a_success_resets_the_count_so_failures_must_be_consecutive() {
    let mut b = breaker();

    fail_times(&mut b, 2, 100);
    assert_eq!(b.permit(100), Permit::Go);
    b.succeeded(100);
    fail_times(&mut b, 2, 101);

    assert_eq!(b.state(), CircuitState::Closed);
}

#[test]
fn an_open_breaker_refuses_and_says_when_to_ask_again() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);

    assert_eq!(b.permit(101), Permit::Refused { retry_at_ns: 100 + OPEN_FOR });
    assert_eq!(b.permit(100 + OPEN_FOR - 1), Permit::Refused { retry_at_ns: 100 + OPEN_FOR });
}

/// A circuit that never tries again would keep a healthy venue closed to the platform forever.
#[test]
fn after_the_open_period_exactly_one_probe_is_let_through() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);
    let reopen = 100 + OPEN_FOR;

    assert_eq!(b.permit(reopen), Permit::Probe);
    assert_eq!(b.state(), CircuitState::HalfOpen);
    assert!(
        matches!(b.permit(reopen + 1), Permit::Refused { .. }),
        "a second caller must wait for the probe"
    );
}

#[test]
fn a_successful_probe_closes_the_breaker() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);
    assert_eq!(b.permit(100 + OPEN_FOR), Permit::Probe);

    b.succeeded(100 + OPEN_FOR + 5);

    assert_eq!(b.state(), CircuitState::Closed);
    assert_eq!(b.permit(100 + OPEN_FOR + 6), Permit::Go);
}

#[test]
fn a_failed_probe_opens_it_again_for_a_full_period() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);
    let probe_at = 100 + OPEN_FOR;
    assert_eq!(b.permit(probe_at), Permit::Probe);

    b.failed(probe_at + 5);

    assert_eq!(b.state(), CircuitState::Open { until_ns: probe_at + 5 + OPEN_FOR });
}

/// A probe that never reports (its caller died) must not wedge the breaker half-open.
#[test]
fn a_probe_that_never_reports_is_replaced_after_a_period() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);
    let probe_at = 100 + OPEN_FOR;
    assert_eq!(b.permit(probe_at), Permit::Probe);

    assert!(matches!(b.permit(probe_at + OPEN_FOR - 1), Permit::Refused { .. }));
    assert_eq!(b.permit(probe_at + OPEN_FOR), Permit::Probe);
}

/// A call that began before the breaker opened can finish after it did. Its result is stale.
#[test]
fn a_late_success_does_not_close_an_open_breaker() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);

    b.succeeded(105);

    assert_eq!(b.state(), CircuitState::Open { until_ns: 100 + OPEN_FOR });
}

#[test]
fn a_late_failure_does_not_extend_an_open_breaker() {
    let mut b = breaker();
    fail_times(&mut b, 3, 100);

    b.failed(500);

    assert_eq!(b.state(), CircuitState::Open { until_ns: 100 + OPEN_FOR });
}
