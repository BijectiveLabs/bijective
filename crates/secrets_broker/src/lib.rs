// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Secrets broker - the only path from a component to a credential.
//! Design and rationale: `docs/concepts/secrets.md`.

pub mod credential;
pub mod keystore;
pub mod source;

pub use credential::{Credential, KeyMaterial};
pub use keystore::{Keystore, KeystoreError};
pub use source::{DeclaredSource, InitCredentialSource, KeySource, PassphraseSource, SourceError};

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
