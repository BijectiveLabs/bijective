// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operator registry file: who may sign commands, and with which role.

use bijective_authority::Role;
use bijective_identity::SigningKey;

use super::parse_operators;

fn public_hex(seed_byte: u8) -> String {
    let key = SigningKey::from_seed(&[seed_byte; 32]).expect("seed");
    bijective_identity::to_hex(&key.public_key())
}

fn file(entries: &str) -> String {
    format!(r#"{{ "version": 1, "operators": [ {entries} ] }}"#)
}

fn entry(name: &str, role: &str, public_key: &str) -> String {
    format!(r#"{{ "name": "{name}", "role": "{role}", "public_key": "{public_key}" }}"#)
}

#[test]
fn a_registry_file_names_each_operator_with_a_role_and_a_key() {
    let raw = file(
        &[entry("ada", "operator", &public_hex(1)), entry("bo", "risk-officer", &public_hex(2))]
            .join(","),
    );
    let registry = parse_operators(raw.as_bytes()).expect("registry");
    let signature = SigningKey::from_seed(&[2; 32]).unwrap().sign(b"m");
    assert_eq!(
        registry.authenticate("bo", Role::RiskOfficer, b"m", Some(&signature)),
        Ok(Role::RiskOfficer)
    );
}

#[test]
fn a_role_this_build_does_not_know_is_refused() {
    let raw = file(&entry("ada", "administrator", &public_hex(1)));
    let said = parse_operators(raw.as_bytes()).expect_err("unknown role");
    assert!(said.contains("administrator"), "{said}");
}

#[test]
fn a_key_that_is_not_64_hex_characters_is_refused() {
    let raw = file(&entry("ada", "operator", "abcd"));
    assert!(parse_operators(raw.as_bytes()).is_err());
}

#[test]
fn a_name_that_repeats_and_an_empty_list_are_refused() {
    let twice = file(
        &[entry("ada", "operator", &public_hex(1)), entry("ada", "observer", &public_hex(2))]
            .join(","),
    );
    assert!(parse_operators(twice.as_bytes()).expect_err("repeat").contains("ada"));
    assert!(parse_operators(file("").as_bytes()).is_err());
}

#[test]
fn a_file_of_another_version_is_refused() {
    let raw =
        file(&entry("ada", "operator", &public_hex(1))).replace("\"version\": 1", "\"version\": 2");
    assert!(parse_operators(raw.as_bytes()).is_err());
}
