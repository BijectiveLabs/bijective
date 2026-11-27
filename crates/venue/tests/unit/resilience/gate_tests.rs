// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{
    BreakerConfig, CallClass, CallGate, CallResult, GateConfig, Held, Quota, call_result_for_http,
    retry_after_from_header,
};

const OPEN_FOR: i128 = 30_000_000_000;
const SECOND: i128 = 1_000_000_000;

fn gate() -> CallGate {
    CallGate::new(GateConfig {
        breaker: BreakerConfig { failures_to_open: 3, open_for_ns: OPEN_FOR },
        reads: Quota { requests: 5, per_ns: SECOND },
    })
    .expect("a valid gate")
}

fn fail(g: &mut CallGate, class: CallClass, times: u32, at: i128) {
    for _ in 0..times {
        g.admit(class, at).expect("admitted");
        g.report(class, at, CallResult::Unhealthy);
    }
}

#[test]
fn a_gate_refuses_a_configuration_that_cannot_work() {
    let bad_breaker = GateConfig {
        breaker: BreakerConfig { failures_to_open: 0, open_for_ns: OPEN_FOR },
        reads: Quota { requests: 5, per_ns: SECOND },
    };
    let bad_quota = GateConfig {
        breaker: BreakerConfig { failures_to_open: 3, open_for_ns: OPEN_FOR },
        reads: Quota { requests: 0, per_ns: SECOND },
    };

    assert!(CallGate::new(bad_breaker).is_err());
    assert!(CallGate::new(bad_quota).is_err());
}

/// An exit must always be attempted: a platform that cannot reach the venue for new orders still
/// has to be able to cancel.
#[test]
fn a_cancel_is_never_held() {
    let mut g = gate();
    fail(&mut g, CallClass::Submit, 3, 0);
    fail(&mut g, CallClass::Read, 3, 0);
    assert!(g.admit(CallClass::Submit, 0).is_err() && g.admit(CallClass::Read, 0).is_err());

    for n in 0..100 {
        assert_eq!(g.admit(CallClass::Cancel, n), Ok(()), "cancel {n}");
        g.report(CallClass::Cancel, n, CallResult::Unhealthy);
    }
}

#[test]
fn consecutive_unhealthy_submits_open_the_submit_breaker_and_say_when_to_ask_again() {
    let mut g = gate();

    fail(&mut g, CallClass::Submit, 3, 100);

    assert_eq!(
        g.admit(CallClass::Submit, 101),
        Err(Held::CircuitOpen { retry_at_ns: 100 + OPEN_FOR })
    );
}

#[test]
fn a_failing_submit_path_does_not_stop_reads_and_a_failing_read_path_does_not_stop_submits() {
    let mut g = gate();
    fail(&mut g, CallClass::Submit, 3, 0);
    assert_eq!(g.admit(CallClass::Read, 1), Ok(()), "reads are a separate path");

    let mut h = gate();
    fail(&mut h, CallClass::Read, 3, 0);
    assert_eq!(h.admit(CallClass::Submit, 1), Ok(()), "submits are a separate path");
}

#[test]
fn after_the_open_period_one_probe_is_admitted_and_its_result_decides() {
    let mut g = gate();
    fail(&mut g, CallClass::Submit, 3, 0);

    assert_eq!(g.admit(CallClass::Submit, OPEN_FOR), Ok(()), "the probe");
    assert!(g.admit(CallClass::Submit, OPEN_FOR + 1).is_err(), "only one probe");
    g.report(CallClass::Submit, OPEN_FOR + 2, CallResult::Healthy);

    assert_eq!(g.admit(CallClass::Submit, OPEN_FOR + 3), Ok(()), "the breaker closed");
}

#[test]
fn reads_are_held_over_their_quota_and_say_how_long_to_wait() {
    let mut g = gate();

    for n in 0..5 {
        assert_eq!(g.admit(CallClass::Read, 0), Ok(()), "read {n}");
        g.report(CallClass::Read, 0, CallResult::Healthy);
    }

    assert_eq!(g.admit(CallClass::Read, 0), Err(Held::OverBudget { wait_ns: SECOND / 5 }));
}

/// The control engine bounds submits by the venue's order rate. The read quota is not theirs.
#[test]
fn submits_are_not_charged_to_the_read_quota() {
    let mut g = gate();
    for _ in 0..5 {
        g.admit(CallClass::Read, 0).expect("read");
    }

    for n in 0..20 {
        assert_eq!(g.admit(CallClass::Submit, 0), Ok(()), "submit {n}");
        g.report(CallClass::Submit, 0, CallResult::Healthy);
    }
}

#[test]
fn a_venue_retry_after_holds_reads_for_that_long() {
    let mut g = gate();
    g.admit(CallClass::Read, 0).expect("read");

    g.report(CallClass::Read, 0, CallResult::Throttled { retry_after_ns: Some(10 * SECOND) });

    assert_eq!(g.admit(CallClass::Read, SECOND), Err(Held::OverBudget { wait_ns: 9 * SECOND }));
    assert_eq!(g.admit(CallClass::Read, 10 * SECOND), Ok(()));
}

/// The limit is the venue's, not the endpoint's: a cancel that was throttled tells reads to wait too.
#[test]
fn a_throttled_cancel_holds_reads_but_not_itself() {
    let mut g = gate();
    g.admit(CallClass::Cancel, 0).expect("cancel");

    g.report(CallClass::Cancel, 0, CallResult::Throttled { retry_after_ns: Some(10 * SECOND) });

    assert!(g.admit(CallClass::Read, SECOND).is_err());
    assert_eq!(g.admit(CallClass::Cancel, SECOND), Ok(()));
}

/// A throttled answer proves the venue is alive. Opening the breaker on it would stop orders for
/// a venue that is only asking for patience.
#[test]
fn throttling_never_opens_a_breaker() {
    let mut g = gate();
    for _ in 0..50 {
        g.admit(CallClass::Submit, 0).expect("admitted");
        g.report(CallClass::Submit, 0, CallResult::Throttled { retry_after_ns: None });
    }

    assert_eq!(g.admit(CallClass::Submit, 1), Ok(()));
}

#[test]
fn a_healthy_call_resets_the_failure_count() {
    let mut g = gate();
    fail(&mut g, CallClass::Submit, 2, 0);
    g.admit(CallClass::Submit, 0).expect("admitted");
    g.report(CallClass::Submit, 0, CallResult::Healthy);
    fail(&mut g, CallClass::Submit, 2, 1);

    assert_eq!(g.admit(CallClass::Submit, 2), Ok(()), "four failures, never three in a row");
}

#[test]
fn a_retry_after_in_whole_seconds_is_read() {
    assert_eq!(retry_after_from_header("5"), Some(5 * SECOND));
    assert_eq!(retry_after_from_header(" 120 "), Some(120 * SECOND));
}

/// A date form, a negative, a zero and text say nothing the gate can use.
#[test]
fn a_retry_after_that_is_not_a_positive_number_of_seconds_says_nothing() {
    for value in ["Wed, 21 Oct 2026 07:28:00 GMT", "-1", "0", "", "soon", "1.5"] {
        assert_eq!(retry_after_from_header(value), None, "{value:?}");
    }
}

/// A broken or hostile header must not hold reads for days.
#[test]
fn a_retry_after_is_capped_at_an_hour() {
    assert_eq!(retry_after_from_header("3600"), Some(3600 * SECOND));
    assert_eq!(retry_after_from_header("99999999"), Some(3600 * SECOND));
    assert_eq!(retry_after_from_header("18446744073709551616"), Some(3600 * SECOND));
}

#[test]
fn an_http_status_is_read_for_what_it_says_about_the_venue() {
    assert_eq!(call_result_for_http(200, None), CallResult::Healthy);
    assert_eq!(call_result_for_http(400, None), CallResult::Healthy, "a refusal is an answer");
    assert_eq!(call_result_for_http(401, None), CallResult::Healthy);
    assert_eq!(call_result_for_http(0, None), CallResult::Unhealthy, "no answer");
    assert_eq!(call_result_for_http(500, None), CallResult::Unhealthy);
    assert_eq!(call_result_for_http(503, None), CallResult::Unhealthy);
    assert_eq!(
        call_result_for_http(429, Some(7 * SECOND)),
        CallResult::Throttled { retry_after_ns: Some(7 * SECOND) }
    );
    assert_eq!(call_result_for_http(429, None), CallResult::Throttled { retry_after_ns: None });
}
