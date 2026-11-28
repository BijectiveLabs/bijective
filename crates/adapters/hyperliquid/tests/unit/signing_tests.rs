// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Pinned against published vectors (EIP-1052, the EIP-712 worked example). The venue's own page
//! says a local check is not enough.

use super::*;
use k256::ecdsa::{RecoveryId, Signature as EcdsaSignature, SigningKey, VerifyingKey};

fn unhex(text: &str) -> Vec<u8> {
    let text = text.strip_prefix("0x").unwrap_or(text);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Derived here: in the module it would be exercised only by tests, which gate 31 refuses.
fn address_of(key: &VerifyingKey) -> String {
    let point = key.to_sec1_point(false);
    hex(&keccak256(&point.as_bytes()[1..])[12..])
}

/// EIP-1052: "the keccak256 hash of empty data".
#[test]
fn keccak_matches_the_constant_the_standard_publishes() {
    assert_eq!(
        hex(&keccak256(b"")),
        "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
    );
}

/// Keccak-256 and SHA3-256 differ only in padding. The wrong one gives plausible digests the
/// venue rejects.
#[test]
fn keccak_is_not_sha3() {
    let sha3_of_empty = "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a";
    assert_ne!(hex(&keccak256(b"")), sha3_of_empty);
}

/// EIP-712 worked example. Nesting is built here because this adapter signs one flat struct.
#[test]
fn the_standards_own_example_recovers_the_address_it_publishes() {
    let person = type_hash("Person(string name,address wallet)");
    let mut from = Vec::new();
    from.extend_from_slice(&person);
    from.extend_from_slice(&keccak256(b"Cow"));
    from.extend_from_slice(&word(&unhex("CD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826")));
    let from = keccak256(&from);

    let mut to = Vec::new();
    to.extend_from_slice(&person);
    to.extend_from_slice(&keccak256(b"Bob"));
    to.extend_from_slice(&word(&unhex("bBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB")));
    let to = keccak256(&to);

    let mut mail = Vec::new();
    mail.extend_from_slice(&type_hash(
        "Mail(Person from,Person to,string contents)Person(string name,address wallet)",
    ));
    mail.extend_from_slice(&from);
    mail.extend_from_slice(&to);
    mail.extend_from_slice(&keccak256(b"Hello, Bob!"));
    let struct_hash = keccak256(&mail);

    let mut domain = Vec::new();
    domain.extend_from_slice(&type_hash(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    domain.extend_from_slice(&keccak256(b"Ether Mail"));
    domain.extend_from_slice(&keccak256(b"1"));
    domain.extend_from_slice(&word(&1u64.to_be_bytes()));
    domain.extend_from_slice(&word(&unhex("CcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC")));
    let domain_separator = keccak256(&domain);

    let digest = typed_data_digest(&domain_separator, &struct_hash);

    let published = unhex(
        "4355c47d63924e8a72e509b65029052eb6c299d53a04e167c5775fd466751c9d\
         07299936d304c153f6443dfa05f40ff007d72911b6f72307f996231605b91562\
         1c",
    );
    let signature = EcdsaSignature::from_slice(&published[..64]).expect("a signature");
    let recovery = RecoveryId::from_byte(published[64] - 27).expect("a recovery id");
    let key = VerifyingKey::recover_from_prehash(&digest, &signature, recovery)
        .expect("the published signature recovers a key");

    assert_eq!(address_of(&key), "cd2a3d9f938e13cd947ec05abc7fe734df8dd826");
}

/// The vault marker is positional. Dropping it shifts everything after it.
#[test]
fn an_absent_vault_still_contributes_its_byte() {
    let with_marker = action_hash(b"action", 7);
    let mut without = Vec::from(*b"action");
    without.extend_from_slice(&7u64.to_be_bytes());
    assert_ne!(hex(&with_marker), hex(&keccak256(&without)));
}

/// Signing for one environment on the other must not pass silently.
#[test]
fn the_two_environments_do_not_share_a_struct_hash() {
    let connection = [3u8; 32];
    assert_ne!(
        hex(&agent_struct_hash(&connection, true)),
        hex(&agent_struct_hash(&connection, false))
    );
}

/// Proves the recovery id names our own key. Says nothing about the payload.
#[test]
fn a_signature_recovers_the_address_that_made_it() {
    let signer = Signer::from_secret(&[7u8; 32]).expect("a key");
    let digest = keccak256(b"anything");
    let signed = signer.sign(&digest).expect("signed");

    let mut raw = Vec::with_capacity(64);
    raw.extend_from_slice(&signed.r);
    raw.extend_from_slice(&signed.s);
    let signature = EcdsaSignature::from_slice(&raw).expect("a signature");
    let recovery = RecoveryId::from_byte(signed.v - 27).expect("a recovery id");
    let key = VerifyingKey::recover_from_prehash(&digest, &signature, recovery).expect("recovered");

    let same = SigningKey::from_slice(&[7u8; 32]).expect("a key");
    assert_eq!(address_of(&key), address_of(same.verifying_key()));
}

/// A match, not an equality: `Signer` has no `Debug` by design.
#[test]
fn a_key_the_curve_refuses_is_refused_here() {
    match Signer::from_secret(&[0u8; 32]) {
        Err(SigningError::KeyRefused) => {}
        Ok(_) => panic!("a zero scalar was accepted as a wallet key"),
    }
}
