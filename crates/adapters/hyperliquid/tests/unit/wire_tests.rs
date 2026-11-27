// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Bytes checked against the msgpack encoding table, not a second implementation.

use super::*;

fn scale(digits: u8) -> Scale {
    Scale::new(digits).expect("a scale this narrow is accepted")
}

#[test]
fn a_small_map_packs_as_the_table_says() {
    let value =
        Value::Map(vec![("a", Value::Int(0)), ("b", Value::Bool(true)), ("c", Value::str("x"))]);
    assert_eq!(
        pack(&value),
        vec![
            0x83, // fixmap, three pairs
            0xa1, b'a', 0x00, // fixstr "a", positive fixint 0
            0xa1, b'b', 0xc3, // fixstr "b", true
            0xa1, b'c', 0xa1, b'x', // fixstr "c", fixstr "x"
        ]
    );
}

#[test]
fn an_array_packs_as_the_table_says() {
    assert_eq!(
        pack(&Value::Array(vec![Value::Int(1), Value::Bool(false)])),
        vec![0x92, 0x01, 0xc2]
    );
}

/// Same pairs in a different order sign differently. The venue reports it as a missing account.
#[test]
fn field_order_changes_the_bytes() {
    let one = Value::Map(vec![("a", Value::Int(1)), ("b", Value::Int(2))]);
    let other = Value::Map(vec![("b", Value::Int(2)), ("a", Value::Int(1))]);
    assert_ne!(pack(&one), pack(&other));
}

/// Pinned so an unintended builder change fails a test instead of a live order.
#[test]
fn a_placement_carries_its_fields_in_the_venues_order() {
    let action = order_action(&[Placement {
        asset: 0,
        buy: true,
        price: "1".to_string(),
        size: "2".to_string(),
        reduce_only: false,
        client_id: "0x01",
    }]);
    let json = to_json(&action);
    let keys: Vec<usize> = ["\"a\"", "\"b\"", "\"p\"", "\"s\"", "\"r\"", "\"t\"", "\"c\""]
        .iter()
        .map(|key| json.find(key).unwrap_or_else(|| panic!("{key} is absent from {json}")))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "the fields are not in the venue's order: {json}");
}

#[test]
fn an_order_action_names_itself_and_carries_a_grouping() {
    let action = order_action(&[Placement {
        asset: 3,
        buy: false,
        price: "10".to_string(),
        size: "1".to_string(),
        reduce_only: true,
        client_id: "0x02",
    }]);
    let json = to_json(&action);
    assert!(json.starts_with("{\"type\":\"order\","), "{json}");
    assert!(json.contains("\"grouping\":\"na\""), "{json}");
    assert!(json.contains("\"tif\":\"Gtc\""), "{json}");
}

#[test]
fn the_packed_and_the_written_forms_carry_the_same_identifier() {
    let action = cancel_action(7, "0xdeadbeef");
    assert!(to_json(&action).contains("0xdeadbeef"));
    let packed = pack(&action);
    let needle = b"0xdeadbeef";
    assert!(
        packed.windows(needle.len()).any(|w| w == needle),
        "the identifier is not in the packed bytes"
    );
}

#[test]
fn a_cancel_is_keyed_by_our_identifier_and_not_the_venues() {
    let json = to_json(&cancel_action(1, "0x03"));
    assert!(json.contains("\"type\":\"cancelByCloid\""), "{json}");
    assert!(json.contains("\"cloid\":\"0x03\""), "{json}");
    assert!(!json.contains("\"oid\""), "{json}");
}

#[test]
fn trailing_zeroes_are_trimmed_because_the_venue_refuses_them() {
    assert_eq!(wire_decimal(12_300, scale(2)), "123");
    assert_eq!(wire_decimal(12_345, scale(2)), "123.45");
    assert_eq!(wire_decimal(12_340, scale(2)), "123.4");
}

#[test]
fn a_zero_is_written_as_one_character() {
    assert_eq!(wire_decimal(0, scale(2)), "0");
    assert_eq!(wire_decimal(0, scale(0)), "0");
}

#[test]
fn a_negative_keeps_its_sign_and_loses_its_zeroes() {
    assert_eq!(wire_decimal(-12_300, scale(2)), "-123");
    assert_eq!(wire_decimal(-1, scale(2)), "-0.01");
}

#[test]
fn a_scale_of_zero_has_no_point_to_trim() {
    assert_eq!(wire_decimal(1_200, scale(0)), "1200");
}

#[test]
fn a_string_with_a_quote_in_it_does_not_escape_the_body() {
    let json = to_json(&Value::str("a\"b\\c"));
    assert_eq!(json, "\"a\\\"b\\\\c\"");
}
