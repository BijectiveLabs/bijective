// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Signing and verification of operator commands, and the registry that names who may sign.

use super::*;

/// RFC 8032, section 7.1, test 1: an empty message under a published seed. Pins the algorithm to
/// the standard, not to this crate's own round trip.
const RFC8032_SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const RFC8032_PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const RFC8032_SIGNATURE: &str = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";

fn key(seed_byte: u8) -> SigningKey {
    SigningKey::from_seed(&[seed_byte; SEED_LEN]).expect("a seed of the right length")
}

fn registry() -> OperatorRegistry {
    OperatorRegistry::new([
        ("ada".to_string(), Operator { role: Role::Operator, public_key: key(1).public_key() }),
        ("bo".to_string(), Operator { role: Role::RiskOfficer, public_key: key(2).public_key() }),
    ])
    .expect("a registry")
}

#[test]
fn the_algorithm_is_ed25519_as_the_standard_publishes_it() {
    let key = SigningKey::from_seed(&RFC8032_SEED).expect("seed");
    assert_eq!(to_hex(&key.public_key()), RFC8032_PUBLIC);
    assert_eq!(to_hex(&key.sign(b"")), RFC8032_SIGNATURE);
}

#[test]
fn a_registered_operators_signature_yields_the_registered_role() {
    let message = b"halt";
    let signature = key(1).sign(message);
    assert_eq!(
        registry().authenticate("ada", Role::Operator, message, Some(&signature)),
        Ok(Role::Operator)
    );
}

#[test]
fn a_command_with_no_signature_is_refused() {
    assert_eq!(
        registry().authenticate("ada", Role::Operator, b"halt", None),
        Err(AuthFailure::Unsigned)
    );
}

#[test]
fn a_name_the_registry_does_not_hold_is_refused() {
    let signature = key(1).sign(b"halt");
    assert_eq!(
        registry().authenticate("mallory", Role::Operator, b"halt", Some(&signature)),
        Err(AuthFailure::UnknownOperator)
    );
}

/// A signature from another registered operator's key is not the claimed operator's.
#[test]
fn a_signature_by_another_key_is_refused() {
    let signature = key(2).sign(b"halt");
    assert_eq!(
        registry().authenticate("ada", Role::Operator, b"halt", Some(&signature)),
        Err(AuthFailure::BadSignature)
    );
}

#[test]
fn a_changed_message_is_refused() {
    let signature = key(1).sign(b"halt");
    assert_eq!(
        registry().authenticate("ada", Role::Operator, b"resume", Some(&signature)),
        Err(AuthFailure::BadSignature)
    );
}

/// The role a command claims is checked against the registry, never taken from the command: a
/// signed operator command that says it is a risk officer's would otherwise raise itself.
#[test]
fn a_claimed_role_that_differs_from_the_registered_one_is_refused() {
    let signature = key(1).sign(b"change limit");
    assert_eq!(
        registry().authenticate("ada", Role::RiskOfficer, b"change limit", Some(&signature)),
        Err(AuthFailure::RoleMismatch { registered: Role::Operator, claimed: Role::RiskOfficer })
    );
}

#[test]
fn a_registry_holds_each_name_once_and_at_least_one() {
    let operator = Operator { role: Role::Operator, public_key: key(1).public_key() };
    assert_eq!(OperatorRegistry::new([]), Err(IdentityError::NoOperators));
    assert_eq!(
        OperatorRegistry::new([
            ("ada".to_string(), operator.clone()),
            ("ada".to_string(), operator)
        ]),
        Err(IdentityError::DuplicateOperator { name: "ada".to_string() })
    );
}

#[test]
fn hex_round_trips_and_refuses_what_is_not_the_right_length() {
    let public = key(3).public_key();
    assert_eq!(from_hex_32(&to_hex(&public)), Ok(public));
    assert_eq!(from_hex_32("zz"), Err(IdentityError::NotHex));
    assert_eq!(from_hex_32("abcd"), Err(IdentityError::NotHex));
}

#[test]
fn a_generated_key_is_new_each_time_and_can_sign() {
    let (first_seed, first) = SigningKey::generate().expect("random");
    let (second_seed, _) = SigningKey::generate().expect("random");
    assert_ne!(first_seed, second_seed);
    let signature = first.sign(b"x");
    assert!(verify(&first.public_key(), b"x", &signature));
}

#[test]
fn a_signing_key_does_not_print_itself() {
    assert!(!format!("{:?}", key(1)).contains("01"));
}
