// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue answers and what each one settles.

use super::*;

fn ok(statuses: &str) -> String {
    format!(
        "{{\"status\":\"ok\",\"response\":{{\"type\":\"order\",\"data\":{{\"statuses\":{statuses}}}}}}}"
    )
}

#[test]
fn a_resting_order_is_accepted() {
    assert_eq!(read_answer(200, &ok("[{\"resting\":{\"oid\":7}}]")), VenueOutcome::Accepted);
}

#[test]
fn a_filled_order_is_accepted() {
    assert_eq!(
        read_answer(200, &ok("[{\"filled\":{\"oid\":7,\"totalSz\":\"1\"}}]")),
        VenueOutcome::Accepted
    );
}

/// Two bare strings carry no order id until the order arms.
#[test]
fn an_order_waiting_to_arm_is_accepted_even_with_no_identifier() {
    assert_eq!(read_answer(200, &ok("[\"waitingForTrigger\"]")), VenueOutcome::Accepted);
    assert_eq!(read_answer(200, &ok("[\"waitingForFill\"]")), VenueOutcome::Accepted);
}

/// Authentication precedes matching, so the order did not happen. An unknown would send
/// every signing defect to reconciliation.
#[test]
fn a_missing_account_is_a_refusal_and_not_an_unknown() {
    let body =
        "{\"status\":\"err\",\"response\":\"L1 error: User or API Wallet 0x01 does not exist.\"}";
    assert_eq!(read_answer(200, body), VenueOutcome::Rejected(Rejection::CredentialRefused));
    let deposit =
        "{\"status\":\"err\",\"response\":\"Must deposit before performing actions. User: 0x01\"}";
    assert_eq!(read_answer(200, deposit), VenueOutcome::Rejected(Rejection::CredentialRefused));
}

/// Guessing the nearest known rejection is how a platform answers confidently and wrongly.
#[test]
fn a_rejection_this_build_has_no_name_for_carries_the_venues_own_code() {
    for code in [
        "PositionIncreaseAtOpenInterestCap",
        "PositionFlipAtOpenInterestCap",
        "TooAggressiveAtOpenInterestCap",
        "OpenInterestIncrease",
        "PerpMaxPosition",
        "MarketOrderNoLiquidity",
        "InsufficientSpotBalance",
    ] {
        let body = ok(&format!("[{{\"error\":\"{code}\"}}]"));
        match read_answer(200, &body) {
            VenueOutcome::Rejected(Rejection::Unclassified { venue_code }) => {
                assert_eq!(venue_code, code);
            }
            other => panic!("{code} was mapped to something else: {other:?}"),
        }
    }
}

/// The three venue rejections that do map to a platform rejection.
#[test]
fn the_three_named_rejections_land_where_the_reference_puts_them() {
    let cases = [
        ("Oracle", Rejection::PriceOutOfBand),
        ("PerpMargin", Rejection::InsufficientMargin),
        ("MinTradeNtl", Rejection::BelowMinimumSize),
    ];
    for (code, expected) in cases {
        let body = ok(&format!("[{{\"error\":\"{code}\"}}]"));
        assert_eq!(read_answer(200, &body), VenueOutcome::Rejected(expected));
    }
}

#[test]
fn an_error_inside_a_successful_envelope_is_still_a_refusal() {
    let body = ok("[{\"error\":\"Order price too far from oracle\"}]");
    assert_eq!(read_answer(200, &body), VenueOutcome::Rejected(Rejection::PriceOutOfBand));
}

/// With no attribution on offer, the accurate answer is unknown.
#[test]
fn an_envelope_that_names_no_outcome_settles_nothing() {
    let body = "{\"status\":\"ok\",\"response\":{\"type\":\"order\",\"data\":{}}}";
    assert_eq!(read_answer(200, body), VenueOutcome::Unknown(Unknown::Unreadable));
}

#[test]
fn more_outcomes_than_items_settles_nothing() {
    let body = ok("[{\"resting\":{\"oid\":1}},{\"resting\":{\"oid\":2}}]");
    assert_eq!(read_answer(200, &body), VenueOutcome::Unknown(Unknown::Unreadable));
}

#[test]
fn a_body_that_cannot_be_read_settles_nothing() {
    assert_eq!(read_answer(500, "<html>"), VenueOutcome::Unknown(Unknown::Unreadable));
}

/// An unseen shape is not a refusal. The order may be resting.
#[test]
fn an_outcome_shape_this_build_does_not_carry_settles_nothing() {
    let body = ok("[{\"somethingNew\":{\"oid\":1}}]");
    assert_eq!(read_answer(200, &body), VenueOutcome::Unknown(Unknown::Unreadable));
}

#[test]
fn every_refusal_is_known_not_to_have_happened() {
    let body = ok("[{\"error\":\"PerpMargin\"}]");
    assert!(read_answer(200, &body).is_known_not_to_have_happened());
    let unknown = ok("[]");
    assert!(!read_answer(200, &unknown).is_known_not_to_have_happened());
}
