// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

fn every_outcome() -> Vec<Outcome> {
    vec![
        Outcome::Refused,
        Outcome::Submitted,
        Outcome::Accepted,
        Outcome::Rejected,
        Outcome::Unknown,
        Outcome::Filled { fill_id: "f-1".into(), quantity_scaled: 1_500, price_scaled: 2_654_375 },
        Outcome::CancelRequested,
        Outcome::Canceled,
        Outcome::Expired,
        Outcome::Reconciling,
    ]
}

#[test]
fn every_outcome_round_trips_with_and_without_an_order() {
    for outcome in every_outcome() {
        for client_id in [None, Some("c-1".to_string())] {
            let feedback = Feedback {
                proposal_position: 7,
                client_id,
                outcome: outcome.clone(),
                reason: "why".into(),
                control_position: 12,
                at_ns: -3,
            };
            let bytes = encode(&feedback).expect("encode");
            assert_eq!(bytes, encode(&feedback).expect("encode"), "encoding is stable");
            assert_eq!(decode(&bytes).expect("decode"), feedback);
        }
    }
}

#[test]
fn an_outcome_this_build_does_not_know_is_refused() {
    let mut w = Writer::new(ClassId::OrderFeedback, VERSION);
    w.u64(0).u8(ABSENT).u8(99);
    w.str("").expect("detail");
    w.u64(0).i128(0);
    assert!(matches!(
        decode(&w.finish()),
        Err(DecodeError::InvalidValue { field: "outcome", found: 99 })
    ));
}
