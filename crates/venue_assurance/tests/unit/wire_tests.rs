// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use bijective_serialization::signal::fees::{Fees, standing as fees_standing};

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("bijective-assurance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

fn fees(venue: &str) -> Fees {
    Fees {
        venue: venue.to_string(),
        taker: 55_000,
        maker: -15_000,
        source: "the venue's published schedule, read on 2026-09-02".to_string(),
        read_at_ns: 1_788_000_000_000_000_000,
    }
}

/// A check the runner drew from an answer, which is the only kind a fee may rest on.
fn checked(root: &std::path::Path, venue: &str, concluded: Concluded) {
    let outcome = Outcome { venue: venue.to_string(), concluded, at_ns: 7, evidence: None }
        .drawn_from(b"the venue's answer, as it was served");
    let mut w = StreamWriter::open(root, ASSURANCE).expect("open");
    record(&outcome, &mut w).expect("record");
}

/// A conclusion entered by hand: no venue answer behind it.
fn asserted(root: &std::path::Path, venue: &str, concluded: Concluded) {
    let mut w = StreamWriter::open(root, ASSURANCE).expect("open");
    record(&Outcome { venue: venue.to_string(), concluded, at_ns: 7, evidence: None }, &mut w)
        .expect("record");
}

#[test]
fn an_outcome_reads_back_as_what_was_written() {
    for concluded in [
        Concluded::Confirmed,
        Concluded::Diverged { findings: 3, capital_relevant: true },
        Concluded::NotObserved { reason: NotObservedReason::RetrievalMethodGone },
    ] {
        let outcome = Outcome { venue: "drill".into(), concluded, at_ns: 11, evidence: None }
            .drawn_from(b"an answer");

        assert_eq!(decode(&encode(&outcome).expect("encode")).expect("decode"), outcome);
    }
}

#[test]
fn a_fee_for_an_unchecked_venue_is_refused() {
    let root = dir("unchecked");
    match declare_fees(&root, &fees("drill")) {
        Err(DeclareError::Refused(DeclarationRefused::NeverChecked { venue })) => {
            assert_eq!(venue, "drill");
        }
        other => panic!("a fee resting on nothing must be refused: {other:?}"),
    }
}

#[test]
fn a_fee_for_a_venue_whose_check_diverged_is_refused_and_says_so() {
    let root = dir("diverged");
    checked(&root, "drill", Concluded::Diverged { findings: 2, capital_relevant: false });
    match declare_fees(&root, &fees("drill")) {
        Err(DeclareError::Refused(DeclarationRefused::NotConfirmed { concluded, .. })) => {
            assert_eq!(concluded, Concluded::Diverged { findings: 2, capital_relevant: false });
        }
        other => panic!("a divergence is not a confirmation: {other:?}"),
    }
}

#[test]
fn a_fee_for_a_venue_that_could_not_be_observed_is_refused() {
    let root = dir("unobserved");
    checked(&root, "drill", Concluded::NotObserved { reason: NotObservedReason::VenueUnreachable });
    assert!(matches!(
        declare_fees(&root, &fees("drill")),
        Err(DeclareError::Refused(DeclarationRefused::NotConfirmed { .. }))
    ));
}

#[test]
fn a_confirmed_venue_may_have_its_fees_declared() {
    let root = dir("confirmed");
    checked(&root, "drill", Concluded::Confirmed);
    declare_fees(&root, &fees("drill")).expect("declared");

    let read = fees_standing(&root, "drill").expect("read").expect("declared");
    assert_eq!(read.taker, 55_000);
    assert_eq!(read.source, "the venue's published schedule, read on 2026-09-02");
}

#[test]
fn a_confirmation_for_another_venue_does_not_carry() {
    let root = dir("other-venue");
    checked(&root, "elsewhere", Concluded::Confirmed);
    assert!(matches!(
        declare_fees(&root, &fees("drill")),
        Err(DeclareError::Refused(DeclarationRefused::NeverChecked { .. }))
    ));
}

#[test]
fn a_fee_resting_on_a_conclusion_somebody_wrote_down_is_refused() {
    let root = dir("asserted");
    asserted(&root, "drill", Concluded::Confirmed);

    match declare_fees(&root, &fees("drill")) {
        Err(DeclareError::Refused(DeclarationRefused::NotDrawnFromEvidence { venue, .. })) => {
            assert_eq!(venue, "drill");
        }
        other => panic!("a typed confirmation is not a check: {other:?}"),
    }
}

#[test]
fn the_evidence_is_the_answer_it_was_drawn_from() {
    let one = Outcome {
        venue: "drill".into(),
        concluded: Concluded::Confirmed,
        at_ns: 1,
        evidence: None,
    }
    .drawn_from(b"one answer");
    let other = Outcome {
        venue: "drill".into(),
        concluded: Concluded::Confirmed,
        at_ns: 1,
        evidence: None,
    }
    .drawn_from(b"another answer");

    assert!(one.evidence.is_some());
    assert_ne!(one.evidence, other.evidence);
    assert_eq!(decode(&encode(&one).expect("encode")).expect("decode").evidence, one.evidence);
}
