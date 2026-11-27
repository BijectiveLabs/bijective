// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The client's pure pieces: hosts, query encoding, the hash an answer names, and scales.

use bijective_venue::normalise::Scale;

use super::*;

#[test]
fn only_the_production_clob_host_has_a_data_service() {
    assert_eq!(data_endpoint_for("https://clob.polymarket.com/"), Some(DATA_HOST));
    assert_eq!(data_endpoint_for("https://clob-staging.polymarket.com"), None);
}

#[test]
fn a_cursor_is_percent_encoded() {
    assert_eq!(query_value("LTE="), "LTE%3D");
    assert_eq!(query_value("a+b/c_d"), "a%2Bb%2Fc_d");
}

#[test]
fn an_accepted_answer_must_name_the_signed_hash() {
    let body = r#"{"success":true,"orderID":"0xABcd","status":"live"}"#;

    assert!(names_order(body, "0xabcd"));
    assert!(!names_order(body, "0xabce"));
    assert!(!names_order(r#"{"success":true}"#, "0xabcd"));
}

#[test]
fn scales_other_than_the_venues_are_refused() {
    let scale = |digits| Scale::new(digits).expect("scale");

    assert!(scales_of(scale(4), scale(6)).is_ok());
    assert!(scales_of(scale(2), scale(6)).is_err());
}

#[test]
fn a_deposit_wallet_must_be_an_address_other_than_the_keys() {
    let key = [1u8; 20];
    let wallet = "0x2e234dae75c793f67a35089c9d99245e1c58470b";

    assert_eq!(maker_of(key, None).expect("own"), (key, EOA));
    assert_eq!(maker_of(key, Some(wallet)).expect("wallet").1, DEPOSIT_WALLET);
    assert!(maker_of(key, Some("0x0101010101010101010101010101010101010101")).is_err());
    assert!(maker_of(key, Some("2e234dae75c793f67a35089c9d99245e1c58470b")).is_err());
}
