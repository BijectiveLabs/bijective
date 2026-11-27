// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! EIP-712 signing for this venue's order scheme.
//! Recovering the signer locally does not prove the scheme right. Only a venue accepting it does.

use k256::ecdsa::{RecoveryId, SigningKey};
use sha3::{Digest, Keccak256};

const DOMAIN_NAME: &str = "Exchange";
const DOMAIN_VERSION: &str = "1";
const DOMAIN_CHAIN_ID: u64 = 1337;

const SOURCE_MAINNET: &str = "a";
const SOURCE_TESTNET: &str = "b";

const TYPED_DATA_PREFIX: [u8; 2] = [0x19, 0x01];

pub fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

#[must_use]
pub fn type_hash(declaration: &str) -> [u8; 32] {
    keccak256(declaration.as_bytes())
}

#[must_use]
pub fn word(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let start = 32 - bytes.len().min(32);
    out[start..].copy_from_slice(&bytes[bytes.len().saturating_sub(32)..]);
    out
}

/// `keccak256("\x19\x01" || domainSeparator || hashStruct(message))`, as EIP-712 defines signing.
#[must_use]
pub fn typed_data_digest(domain_separator: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
    let mut buffer = Vec::with_capacity(66);
    buffer.extend_from_slice(&TYPED_DATA_PREFIX);
    buffer.extend_from_slice(domain_separator);
    buffer.extend_from_slice(struct_hash);
    keccak256(&buffer)
}

#[must_use]
pub fn exchange_domain_separator() -> [u8; 32] {
    let mut buffer = Vec::with_capacity(160);
    buffer.extend_from_slice(&type_hash(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    buffer.extend_from_slice(&keccak256(DOMAIN_NAME.as_bytes()));
    buffer.extend_from_slice(&keccak256(DOMAIN_VERSION.as_bytes()));
    buffer.extend_from_slice(&word(&DOMAIN_CHAIN_ID.to_be_bytes()));
    // The verifying contract is the zero address, which encodes as a zero word.
    buffer.extend_from_slice(&[0u8; 32]);
    keccak256(&buffer)
}

#[must_use]
pub fn agent_struct_hash(connection_id: &[u8; 32], mainnet: bool) -> [u8; 32] {
    let source = if mainnet { SOURCE_MAINNET } else { SOURCE_TESTNET };
    let mut buffer = Vec::with_capacity(96);
    buffer.extend_from_slice(&type_hash("Agent(string source,bytes32 connectionId)"));
    buffer.extend_from_slice(&keccak256(source.as_bytes()));
    buffer.extend_from_slice(connection_id);
    keccak256(&buffer)
}

/// Markers are positional, so an absent vault still contributes its byte.
#[must_use]
pub fn action_hash(packed_action: &[u8], nonce_ms: u64) -> [u8; 32] {
    let mut buffer = Vec::with_capacity(packed_action.len() + 9);
    buffer.extend_from_slice(packed_action);
    buffer.extend_from_slice(&nonce_ms.to_be_bytes());
    // Zero: no vault. The byte is positional. Dropping it would shift everything after it.
    buffer.push(0x00);
    keccak256(&buffer)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningError {
    KeyRefused,
    NotSigned,
}

impl core::fmt::Display for SigningError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SigningError::KeyRefused => write!(f, "the wallet key is not valid for this curve"),
            SigningError::NotSigned => write!(f, "the digest could not be signed"),
        }
    }
}

impl std::error::Error for SigningError {}

/// No `Debug` and the key is never returned: a printable signer ends up in a log line.
pub struct Signer {
    key: SigningKey,
}

impl Signer {
    /// Takes the key rather than resolving one.
    pub fn from_secret(bytes: &[u8]) -> Result<Self, SigningError> {
        let key = SigningKey::from_slice(bytes).map_err(|_| SigningError::KeyRefused)?;
        Ok(Self { key })
    }

    /// Deterministic nonce from key and message: the key-leaking reuse of one nonce is unreachable.
    pub fn sign(&self, digest: &[u8; 32]) -> Result<Signed, SigningError> {
        let (signature, recovery) =
            self.key.sign_prehash_recoverable(digest).map_err(|_| SigningError::NotSigned)?;
        let bytes = signature.to_bytes();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&bytes[..32]);
        s.copy_from_slice(&bytes[32..]);
        Ok(Signed { r, s, v: recovery_to_v(recovery) })
    }
}

/// Ethereum writes the recovery id as 27 or 28.
fn recovery_to_v(recovery: RecoveryId) -> u8 {
    27 + recovery.to_byte()
}

// Flat file: a submodule would otherwise be looked up in a directory this crate lacks.
#[cfg(test)]
#[path = "../tests/unit/signing_tests.rs"]
mod signing_tests;
