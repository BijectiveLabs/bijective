// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a comparison must refuse to say.

use super::*;
use bijective_model::{Side as OrderSide, VenueSymbol};

fn order(quantity: i128) -> OrderRequest {
    OrderRequest {
        client_id: "abc123".into(),
        instrument: VenueSymbol::new("BTCUSDT"),
        side: OrderSide::Buy,
        quantity_scaled: quantity,
        price_scaled: Some(2_654_375),
        reduce_only: false,
        terms: Default::default(),
    }
}

fn decided(pairs: Vec<(u64, Decision)>) -> BTreeMap<u64, Decision> {
    pairs.into_iter().collect()
}

fn intended(at: u64, quantity: i128) -> (u64, Decision) {
    (at, Decision::Intended(order(quantity)))
}

fn refused(at: u64, reason: RefusalReason) -> (u64, Decision) {
    (at, Decision::Refused(reason))
}

#[test]
fn two_runs_that_decided_the_same_thing_agree() {
    let a = decided(vec![intended(0, 10), refused(1, RefusalReason::Breached)]);
    let b = decided(vec![intended(0, 10), refused(1, RefusalReason::Breached)]);

    let said = compare(&a, &b);
    assert!(said.agreed(), "{said}");
    assert_eq!(said, Comparison::Agreed { positions: 2 });
}

#[test]
fn a_different_order_at_the_same_proposal_is_a_divergence() {
    let a = decided(vec![intended(0, 10)]);
    let b = decided(vec![intended(0, 25)]);

    let said = compare(&a, &b);
    assert!(!said.agreed(), "{said}");
    assert!(
        matches!(
            &said,
            Comparison::Diverged { divergences, .. }
                if matches!(divergences[0], Divergence::DecidedDifferently { at: 0, .. })
        ),
        "{said:?}"
    );
}

#[test]
fn intending_where_the_other_refused_is_a_divergence() {
    let a = decided(vec![intended(0, 10)]);
    let b = decided(vec![refused(0, RefusalReason::Breached)]);

    assert!(!compare(&a, &b).agreed());
}

#[test]
fn a_run_that_stopped_early_does_not_agree_with_one_that_did_not() {
    let a = decided(vec![intended(0, 10), intended(1, 10)]);
    let b = decided(vec![intended(0, 10)]);

    let said = compare(&a, &b);
    assert!(!said.agreed(), "{said}");
    assert!(
        matches!(
            &said,
            Comparison::Diverged { divergences, .. }
                if divergences.contains(&Divergence::OnlyOneAnswered { at: 1, answered_by: Side::Paper })
        ),
        "{said:?}"
    );
}

#[test]
fn two_runs_that_decided_nothing_are_not_comparable() {
    let said = compare(&BTreeMap::new(), &BTreeMap::new());

    assert_eq!(said, Comparison::NotComparable(NotComparable::NeitherDecided));
    assert!(!said.agreed(), "an empty comparison read as agreement");
}

#[test]
fn runs_over_different_proposals_are_not_comparable() {
    let a = decided(vec![intended(0, 10)]);
    let b = decided(vec![intended(90, 10)]);

    let said = compare(&a, &b);
    assert!(!said.agreed(), "{said}");
    assert!(
        matches!(
            said,
            Comparison::NotComparable(NotComparable::NoSharedProposal { paper: 1, live: 1 })
        ),
        "{said:?}"
    );
}

#[test]
fn refusing_for_a_different_reason_is_a_divergence() {
    let a = decided(vec![refused(0, RefusalReason::Breached)]);
    let b = decided(vec![refused(0, RefusalReason::LimitsUnrecovered)]);

    assert!(!compare(&a, &b).agreed());
}

// what `decisions` reads, and what it leaves out

use bijective_control_engine::wire::encode;
use bijective_event_store::store::StreamWriter;
use bijective_venue::normalise::VenueOutcome;

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn root(name: &str) -> std::path::PathBuf {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("bijective-paper-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a place to write");
    dir
}

fn write(dir: &std::path::Path, events: &[ControlEvent]) {
    let mut w = StreamWriter::open(dir, "control").expect("writer");
    for e in events {
        w.append(&encode(e).expect("encode")).expect("append");
    }
}

/// Instant and settlement outcome differ by design between the two runs.
#[test]
fn a_different_instant_and_a_different_outcome_are_not_a_divergence() {
    let a = root("paper");
    let b = root("live");

    write(
        &a,
        &[
            ControlEvent::Intent {
                decision_id: "dec-1".into(),
                venue: Some("venue-a".into()),
                owner: None,
                request: order(10),
                from_position: 0,
                at_ns: 1_000,
            },
            ControlEvent::Settled {
                client_id: "abc123".into(),
                outcome: VenueOutcome::NotSent,
                at_ns: 1_001,
            },
        ],
    );
    write(
        &b,
        &[
            ControlEvent::Intent {
                decision_id: "dec-1".into(),
                venue: Some("venue-a".into()),
                owner: None,
                request: order(10),
                from_position: 0,
                at_ns: 9_999_999,
            },
            ControlEvent::Settled {
                client_id: "abc123".into(),
                outcome: VenueOutcome::Accepted,
                at_ns: 9_999_999,
            },
        ],
    );

    let paper = decisions(&a, "control").expect("read");
    let live = decisions(&b, "control").expect("read");

    let said = compare(&paper, &live);
    assert!(said.agreed(), "the two runs decided the same thing and were reported apart: {said}");
}

#[test]
fn reading_a_stream_still_carries_the_order_that_was_decided() {
    let a = root("carried-a");
    let b = root("carried-b");

    write(
        &a,
        &[ControlEvent::Intent {
            decision_id: "dec-1".into(),
            venue: Some("venue-a".into()),
            owner: None,
            request: order(10),
            from_position: 0,
            at_ns: 1,
        }],
    );
    write(
        &b,
        &[ControlEvent::Intent {
            decision_id: "dec-1".into(),
            venue: Some("venue-a".into()),
            owner: None,
            request: order(25),
            from_position: 0,
            at_ns: 1,
        }],
    );

    let said = compare(&decisions(&a, "control").unwrap(), &decisions(&b, "control").unwrap());
    assert!(!said.agreed(), "a different order survived the read as agreement: {said}");
}
