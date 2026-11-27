// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The resolved credential, and why it cannot be printed.

use core::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A credential resolved through the broker.
/// No secret may reach configuration, source or logs: the value has no rendering that reveals it.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Credential {
    bytes: Vec<u8>,
}

impl Credential {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// The bytes, named so every use site can be found by searching for one word.
    /// Deliberately not `as_bytes` or `Deref`, which would be called without thought.
    pub fn expose(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

// A redacting Debug rather than none, so structs holding a Credential can still derive Debug.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential(redacted, {} bytes)", self.bytes.len())
    }
}

/// Key material for the keystore. Separate from [`Credential`] because it lives longer and
/// unwraps everything else.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct KeyMaterial {
    bytes: [u8; 32],
}

impl KeyMaterial {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    pub fn expose(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyMaterial(redacted)")
    }
}
