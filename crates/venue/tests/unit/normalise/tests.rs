// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Gates for the decimal conversion and the outcome taxonomy.

use super::*;

fn scale(n: u8) -> Scale {
    Scale::new(n).expect("scale within range")
}

#[test]
fn venue_prices_and_quantities_parse_exactly() {
    assert_eq!(parse_scaled("26543.75", scale(2)).unwrap(), 2_654_375);
    assert_eq!(parse_scaled("0.001", scale(3)).unwrap(), 1);
    assert_eq!(parse_scaled("-1250.5", scale(2)).unwrap(), -125_050);
    assert_eq!(parse_scaled("0", scale(8)).unwrap(), 0);
    assert_eq!(parse_scaled("1", scale(0)).unwrap(), 1);
}

#[test]
fn parse_and_render_round_trip() {
    for (text, digits) in [
        ("26543.75", 2u8),
        ("0.00000001", 8),
        ("-0.50", 2),
        ("1000000.000000", 6),
        ("0.000000000000000001", 18),
    ] {
        let s = scale(digits);
        let parsed = parse_scaled(text, s).unwrap();
        assert_eq!(render_scaled(parsed, s), text, "round trip failed for {text}");
    }
}

/// Excess precision is refused, never rounded: a dropped digit makes a benchmark measure a fiction.
#[test]
fn more_precision_than_the_profile_declares_is_refused() {
    let err = parse_scaled("1.234", scale(2)).unwrap_err();
    assert_eq!(err, DecimalError::ExcessPrecision { declared: 2, needed: 3 });
    assert!(err.to_string().contains("profile declares 2"));
}

/// Trailing zeros are not precision, and refusing them would reject a value both sides agree on.
#[test]
fn trailing_zeros_are_not_excess_precision() {
    assert_eq!(parse_scaled("1.2300", scale(2)).unwrap(), 123);
    assert_eq!(parse_scaled("5.000000", scale(0)).unwrap(), 5);
    assert_eq!(parse_scaled("-0.10", scale(1)).unwrap(), -1);
}

#[test]
fn exponent_notation_is_exact() {
    assert_eq!(parse_scaled("1e-8", scale(8)).unwrap(), 1);
    assert_eq!(parse_scaled("1.5E3", scale(2)).unwrap(), 150_000);
    assert_eq!(parse_scaled("-2.5e-2", scale(4)).unwrap(), -250);
    assert_eq!(
        parse_scaled("1e-9", scale(8)).unwrap_err(),
        DecimalError::ExcessPrecision { declared: 8, needed: 9 }
    );
}

/// The prior research code held money as `f64`. This is a satoshi-scale value where
/// that mattered.
#[test]
fn a_float_would_not_have_carried_this_and_our_parser_does() {
    let text = "0.12345678901234567";
    let s = scale(17);
    let exact = parse_scaled(text, s).unwrap();
    assert_eq!(render_scaled(exact, s), text);

    let through_double: f64 = text.parse().unwrap();
    let rounded = format!("{through_double:.17}");
    assert_ne!(
        rounded, text,
        "if a double carried this exactly, the conversion would be unnecessary"
    );
}

#[test]
fn tenths_add_exactly_here_and_not_in_a_double() {
    let s = scale(1);
    let a = parse_scaled("0.1", s).unwrap();
    let b = parse_scaled("0.2", s).unwrap();
    assert_eq!(render_scaled(a + b, s), "0.3");

    let double = 0.1_f64 + 0.2_f64;
    assert_ne!(double, 0.3_f64);
}

#[test]
fn malformed_values_are_refused_rather_than_guessed() {
    assert_eq!(parse_scaled("", scale(2)).unwrap_err(), DecimalError::Empty);
    assert_eq!(parse_scaled("   ", scale(2)).unwrap_err(), DecimalError::Empty);
    assert_eq!(parse_scaled("1.2.3", scale(2)).unwrap_err(), DecimalError::Malformed);
    assert_eq!(parse_scaled(".", scale(2)).unwrap_err(), DecimalError::Malformed);
    assert!(matches!(
        parse_scaled("12a4", scale(2)),
        Err(DecimalError::NotANumber { found: 'a', .. })
    ));
    assert!(matches!(parse_scaled("NaN", scale(2)), Err(DecimalError::NotANumber { .. })));
    assert!(matches!(parse_scaled("Infinity", scale(2)), Err(DecimalError::NotANumber { .. })));
}

/// A value too large for the platform integer is refused, never wrapped.
#[test]
fn a_value_beyond_the_platform_integer_is_refused() {
    let huge = "9".repeat(40);
    assert_eq!(parse_scaled(&huge, scale(0)).unwrap_err(), DecimalError::Overflow);
    assert_eq!(parse_scaled("1e41", scale(0)).unwrap_err(), DecimalError::Overflow);
}

/// The scale itself is bounded, because a scale the integer cannot hold is not a scale.
#[test]
fn a_scale_beyond_the_platforms_width_is_refused() {
    assert!(Scale::new(18).is_ok());

    // Limit and offered value are both stated, so no caller invents the message.
    assert_eq!(Scale::new(19).unwrap_err(), ScaleError::TooWide { asked: 19, maximum: 18 });
    assert!(Scale::new(19).unwrap_err().to_string().contains("at most 18"));
}

/// Zero renders with its declared precision: a venue field may require "0.00".
#[test]
fn zero_carries_its_scale() {
    assert_eq!(render_scaled(0, scale(2)), "0.00");
    assert_eq!(render_scaled(0, scale(0)), "0");
}

#[test]
fn a_small_negative_keeps_its_sign() {
    let s = scale(4);
    let v = parse_scaled("-0.0001", s).unwrap();
    assert_eq!(v, -1);
    assert_eq!(render_scaled(v, s), "-0.0001");
}

use crate::normalise::outcome::{Rejection, Unknown, VenueOutcome};

/// A timeout is not a refusal: the order may have filled.
#[test]
fn an_unknown_is_never_a_rejection() {
    let unknown = VenueOutcome::Unknown(Unknown::Timeout);
    assert!(!unknown.is_known_not_to_have_happened());
    assert!(unknown.needs_reconciliation(Attempted::Submit));

    let rejected = VenueOutcome::Rejected(Rejection::InsufficientMargin);
    assert!(rejected.is_known_not_to_have_happened());
    assert!(!rejected.needs_reconciliation(Attempted::Submit));
}

#[test]
fn every_unknown_shape_reaches_reconciliation() {
    for u in [Unknown::Timeout, Unknown::ConnectionLost, Unknown::Unreadable] {
        let o = VenueOutcome::Unknown(u);
        assert!(o.needs_reconciliation(Attempted::Submit), "{u:?} did not reach reconciliation");
        assert!(!o.is_known_not_to_have_happened());
    }
}

/// An unrecognised code is carried, never mapped to the nearest class, so venue assurance can see a
/// venue that changed.
#[test]
fn an_unrecognised_code_travels_with_the_rejection() {
    let o = VenueOutcome::Rejected(Rejection::Unclassified { venue_code: "110099".into() });
    assert!(o.is_known_not_to_have_happened(), "the venue did refuse");
    match o {
        VenueOutcome::Rejected(Rejection::Unclassified { venue_code }) => {
            assert_eq!(venue_code, "110099");
        }
        other => panic!("the code was lost: {other:?}"),
    }
}

/// Idempotent resubmission is how a client survives an unknown, and only an unknown.
#[test]
fn resubmission_is_permitted_only_where_the_answer_is_unknown() {
    assert!(VenueOutcome::Unknown(Unknown::ConnectionLost).may_resubmit_idempotently());
    assert!(!VenueOutcome::Rejected(Rejection::PriceOutOfBand).may_resubmit_idempotently());
    assert!(!VenueOutcome::Accepted.may_resubmit_idempotently());
}

/// A duplicate is the venue confirming the first attempt landed, not a fault.
#[test]
fn a_duplicate_is_an_answer_rather_than_a_failure() {
    let o = VenueOutcome::Rejected(Rejection::Duplicate);
    assert!(o.is_known_not_to_have_happened(), "this submission did not create a second order");
    assert!(!o.needs_reconciliation(Attempted::Submit), "the venue has answered");
    assert!(!o.leaves_no_order(), "the first attempt is at the venue");
    assert!(VenueOutcome::Rejected(Rejection::BelowMinimumSize).leaves_no_order());
    assert!(!VenueOutcome::Unknown(Unknown::Timeout).leaves_no_order());
    assert!(!VenueOutcome::Accepted.leaves_no_order());
}

/// Rate limiting may or may not say when to retry, and the profile cannot invent it.
#[test]
fn a_rate_limit_without_a_retry_hint_says_so() {
    let with = VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(500_000_000) });
    let without = VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None });
    assert_ne!(with, without);
}

/// A refused submit settles the order. A refused cancel does not, since filling is one way an
/// order stops resting.
#[test]
fn an_unreadable_refusal_means_different_things_for_a_submission_and_a_cancel() {
    let o = VenueOutcome::Rejected(Rejection::Unclassified { venue_code: "110001".to_string() });

    assert!(o.is_known_not_to_have_happened(), "the venue did refuse the request");
    assert!(!o.needs_reconciliation(Attempted::Submit), "a refused submission created no order");
    assert!(
        o.needs_reconciliation(Attempted::Cancel),
        "a refused cancel may be refusing because the order already filled"
    );
}

#[test]
fn a_recognised_refusal_of_a_cancel_stays_out_of_reconciliation() {
    for o in [
        VenueOutcome::Rejected(Rejection::CredentialRefused),
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None }),
    ] {
        assert!(
            !o.needs_reconciliation(Attempted::Cancel),
            "{o:?} names a reason that is not the order leaving the book"
        );
    }
}

#[test]
fn a_rate_limit_rejection_takes_the_venue_retry_after_only_when_it_has_none() {
    let bare = VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None });
    let stated = VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(5) });

    assert_eq!(
        bare.with_retry_after(Some(9)),
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(9) })
    );
    assert_eq!(stated.clone().with_retry_after(Some(9)), stated, "a stated wait is kept");
}

#[test]
fn no_other_outcome_is_changed_by_a_retry_after() {
    for outcome in [
        VenueOutcome::Accepted,
        VenueOutcome::NotSent,
        VenueOutcome::Unknown(Unknown::Timeout),
        VenueOutcome::Rejected(Rejection::InsufficientMargin),
    ] {
        assert_eq!(outcome.clone().with_retry_after(Some(9)), outcome);
    }
}
