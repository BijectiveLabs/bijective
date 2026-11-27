// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The client's pure pieces: the scales it trades on and query encoding.

use bijective_venue::normalise::Scale;

use super::*;

#[test]
fn only_whole_shares_and_a_grid_of_four_places_or_finer_are_traded() {
    let scale = |digits| Scale::new(digits).expect("scale");

    assert!(scales_of(scale(4), scale(0)).is_ok());
    assert!(scales_of(scale(9), scale(0)).is_ok());
    assert!(scales_of(scale(2), scale(0)).is_err(), "a sub-dollar price cannot be held");
    assert!(scales_of(scale(4), scale(9)).is_err(), "fractional shares are not built");
}

#[test]
fn a_client_identifier_is_percent_encoded() {
    assert_eq!(query_value("bl-1_a.b~c"), "bl-1_a.b~c");
    assert_eq!(query_value("a b/c=d"), "a%20b%2Fc%3Dd");
}
