// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Who an operator command came from.
//!
//! An operator holds an Ed25519 signing key. The platform holds the public keys in a registry that
//! also states each operator's role. A command is trusted when its signature verifies under the
//! key the registry holds for the name it carries, and the role it acts with is the registry's,
//! never the one it claims. Precondition: the message signed is the command's canonical bytes
//! without the signature. Postcondition: `authenticate` returns a role only for a verified
//! signature. Every other outcome is a named refusal.

use std::collections::BTreeMap;

use bijective_authority::Role;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

pub const SEED_LEN: usize = 32;
pub const PUBLIC_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The seed was rejected by the signature library.
    SeedRejected,
    /// The system's random source could not produce a key.
    NoRandomness,
    NoOperators,
    DuplicateOperator {
        name: String,
    },
    /// Not 64 hexadecimal characters.
    NotHex,
}

impl core::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SeedRejected => write!(f, "the signing seed was rejected"),
            Self::NoRandomness => write!(f, "the system could not produce random bytes"),
            Self::NoOperators => write!(f, "an operator registry holds at least one operator"),
            Self::DuplicateOperator { name } => write!(f, "{name} is registered twice"),
            Self::NotHex => write!(f, "expected 64 hexadecimal characters"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// An operator's private key. Never printed.
pub struct SigningKey(Ed25519KeyPair);

impl core::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SigningKey(redacted)")
    }
}

impl SigningKey {
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Result<Self, IdentityError> {
        Ed25519KeyPair::from_seed_unchecked(seed).map(Self).map_err(|_| IdentityError::SeedRejected)
    }

    /// A new key from the system's random source, with its seed so the caller can store it.
    pub fn generate() -> Result<([u8; SEED_LEN], Self), IdentityError> {
        let mut seed = [0u8; SEED_LEN];
        ring::rand::SecureRandom::fill(&SystemRandom::new(), &mut seed)
            .map_err(|_| IdentityError::NoRandomness)?;
        let key = Self::from_seed(&seed)?;
        Ok((seed, key))
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; PUBLIC_LEN] {
        let mut out = [0u8; PUBLIC_LEN];
        out.copy_from_slice(self.0.public_key().as_ref());
        out
    }

    /// Deterministic: the same key and message always give the same signature.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_LEN] {
        let mut out = [0u8; SIGNATURE_LEN];
        out.copy_from_slice(self.0.sign(message).as_ref());
        out
    }
}

#[must_use]
pub fn verify(
    public_key: &[u8; PUBLIC_LEN],
    message: &[u8],
    signature: &[u8; SIGNATURE_LEN],
) -> bool {
    UnparsedPublicKey::new(&ED25519, public_key).verify(message, signature).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    pub role: Role,
    pub public_key: [u8; PUBLIC_LEN],
}

/// Why a command was not trusted. Each is recorded with the refusal, so an auditor reads the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFailure {
    Unsigned,
    UnknownOperator,
    BadSignature,
    RoleMismatch { registered: Role, claimed: Role },
}

impl core::fmt::Display for AuthFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unsigned => write!(f, "the command carries no signature"),
            Self::UnknownOperator => write!(f, "the operator is not in the registry"),
            Self::BadSignature => write!(f, "the signature does not verify"),
            Self::RoleMismatch { registered, claimed } => write!(
                f,
                "the command claims the {} role and the registry holds {}",
                claimed.as_str(),
                registered.as_str()
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRegistry {
    operators: BTreeMap<String, Operator>,
}

impl OperatorRegistry {
    /// Refused when empty or when a name repeats: a registry that quietly kept the last entry would
    /// let a later line replace an earlier operator's key.
    pub fn new(
        entries: impl IntoIterator<Item = (String, Operator)>,
    ) -> Result<Self, IdentityError> {
        let mut operators = BTreeMap::new();
        for (name, operator) in entries {
            if operators.insert(name.clone(), operator).is_some() {
                return Err(IdentityError::DuplicateOperator { name });
            }
        }
        if operators.is_empty() {
            return Err(IdentityError::NoOperators);
        }
        Ok(Self { operators })
    }

    pub fn authenticate(
        &self,
        actor: &str,
        claimed_role: Role,
        message: &[u8],
        signature: Option<&[u8; SIGNATURE_LEN]>,
    ) -> Result<Role, AuthFailure> {
        let signature = signature.ok_or(AuthFailure::Unsigned)?;
        let operator = self.operators.get(actor).ok_or(AuthFailure::UnknownOperator)?;
        if !verify(&operator.public_key, message, signature) {
            return Err(AuthFailure::BadSignature);
        }
        if operator.role != claimed_role {
            return Err(AuthFailure::RoleMismatch {
                registered: operator.role,
                claimed: claimed_role,
            });
        }
        Ok(operator.role)
    }
}

#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Exactly 64 hexadecimal characters, the form a key travels in a file.
pub fn from_hex_32(text: &str) -> Result<[u8; 32], IdentityError> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return Err(IdentityError::NotHex);
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| IdentityError::NotHex)?;
    }
    Ok(out)
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
