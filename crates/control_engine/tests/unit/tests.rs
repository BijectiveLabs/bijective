// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

#[test]
fn a_verified_history_brings_the_program_up_and_leaves_trading_halted() {
    let s = assess(Recovery::Verified { through: Some(42) });

    assert!(s.is_up());
    assert!(s.detail().contains("position 42"));
    assert!(s.detail().contains("halted"), "trading state must be stated, not assumed");
}

/// First boot: an empty verified store is up, not broken.
#[test]
fn an_empty_but_verified_history_is_not_a_failure() {
    let s = assess(Recovery::Verified { through: None });

    assert!(s.is_up());
    assert!(s.detail().contains("empty"));
    assert!(s.detail().contains("halted"));
}

/// Risk limits: an incomplete replay must not start the node.
#[test]
fn an_incomplete_replay_refuses_to_come_up() {
    let s = assess(Recovery::Incomplete);

    assert!(!s.is_up());
    assert!(s.detail().contains("did not complete"));
}

#[test]
fn every_outcome_explains_itself() {
    for recovery in [
        Recovery::Verified { through: Some(1) },
        Recovery::Verified { through: None },
        Recovery::Incomplete,
    ] {
        assert!(!assess(recovery).detail().is_empty());
    }
}

use bijective_limits::{Evaluation, Freshness, LimitKind};

/// A clear evaluation yields the key. Anything else yields none.
#[test]
fn only_a_clear_evaluation_yields_a_clearance() {
    let clear = Evaluation { breaches: vec![], unjudged: vec![], freshness: Freshness::Fresh };
    assert!(clear_to_submit(&clear, 1).is_some());
}

/// An unjudged limit is not evidence of safety.
#[test]
fn an_unjudged_limit_yields_no_clearance() {
    let unjudged = Evaluation {
        breaches: vec![],
        unjudged: vec![LimitKind::GrossExposure],
        freshness: Freshness::Fresh,
    };
    assert!(clear_to_submit(&unjudged, 1).is_none());
}

/// Stale figures mean the platform does not know whether it is within limits.
#[test]
fn stale_state_yields_no_clearance_even_with_nothing_breached() {
    let stale = Evaluation {
        breaches: vec![],
        unjudged: vec![],
        freshness: Freshness::Stale { age_ns: 1 },
    };
    assert!(clear_to_submit(&stale, 1).is_none());
}

/// The key and the risk engine's verdict must never disagree.
#[test]
fn the_key_and_the_verdict_agree_on_every_shape() {
    let shapes = [
        Evaluation { breaches: vec![], unjudged: vec![], freshness: Freshness::Fresh },
        Evaluation {
            breaches: vec![],
            unjudged: vec![],
            freshness: Freshness::Stale { age_ns: 5 },
        },
        Evaluation {
            breaches: vec![],
            unjudged: vec![LimitKind::GrossExposure],
            freshness: Freshness::Fresh,
        },
    ];
    for e in &shapes {
        assert_eq!(e.is_clear(), clear_to_submit(e, 1).is_some());
    }
}
