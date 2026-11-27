// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Where the keystore key comes from, and why there is no fallback path.

use crate::credential::KeyMaterial;
use argon2::{Algorithm, Argon2, Params, Version};

/// The sources a deployment may declare.
/// The configured source is the only one tried, with no ordering and no `Default`.
/// Falling back from a sealed source to a passphrase is how a control disappears unnoticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredSource {
    /// The init system's encrypted credentials. Needs systemd 250 or newer.
    InitCredential,
    TpmSeal,
    Passphrase,
}

impl DeclaredSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DeclaredSource::InitCredential => "init-credential",
            DeclaredSource::TpmSeal => "tpm-seal",
            DeclaredSource::Passphrase => "passphrase",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The declared source is not available on this host.
    /// Startup stops here. Trying another source is the fallback the record forbids.
    Unavailable {
        source: DeclaredSource,
        detail: &'static str,
    },
    Failed {
        source: DeclaredSource,
    },
    /// Key derivation parameters outside the accepted range.
    BadParameters,
}

impl core::fmt::Display for SourceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SourceError::Unavailable { source, detail } => {
                write!(f, "declared source {} is unavailable: {detail}", source.as_str())
            }
            SourceError::Failed { source } => {
                write!(f, "declared source {} did not produce a key", source.as_str())
            }
            SourceError::BadParameters => write!(f, "key derivation parameters are out of range"),
        }
    }
}

impl std::error::Error for SourceError {}

/// Resolves one declared source into key material.
/// Asking for a source the host does not implement is [`SourceError::Unavailable`], never a substitution.
pub trait KeySource {
    fn declared(&self) -> DeclaredSource;
    fn unwrap_key(&self) -> Result<KeyMaterial, SourceError>;
}

/// Argon2id parameters, not configurable downward: configuration must not set the strength of every credential.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_PARALLELISM: u32 = 1;

/// The passphrase source, implemented here because it needs nothing from the host.
/// So is the init credential below. Only the TPM source needs host contact.
pub struct PassphraseSource {
    passphrase: Vec<u8>,
    salt: Vec<u8>,
}

impl PassphraseSource {
    /// The salt comes from the keystore header, so a keystore carries everything but the secret.
    pub fn new(passphrase: Vec<u8>, salt: Vec<u8>) -> Self {
        Self { passphrase, salt }
    }
}

impl KeySource for PassphraseSource {
    fn declared(&self) -> DeclaredSource {
        DeclaredSource::Passphrase
    }

    fn unwrap_key(&self) -> Result<KeyMaterial, SourceError> {
        if self.salt.len() < 16 || self.passphrase.is_empty() {
            return Err(SourceError::BadParameters);
        }
        let params = Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_PARALLELISM, Some(32))
            .map_err(|_| SourceError::BadParameters)?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut out = [0u8; 32];
        argon
            .hash_password_into(&self.passphrase, &self.salt, &mut out)
            .map_err(|_| SourceError::Failed { source: DeclaredSource::Passphrase })?;
        Ok(KeyMaterial::from_bytes(out))
    }
}

/// The init system's encrypted credentials.
/// systemd 250+ decrypts `LoadCredentialEncrypted=` into `$CREDENTIALS_DIRECTORY`. The platform never sees the sealing key.
pub struct InitCredentialSource {
    name: String,
    directory: Option<std::path::PathBuf>,
}

impl InitCredentialSource {
    /// Read the directory the init system named.
    /// A missing directory is kept, so the refusal happens where the key is asked for and can name what was absent.
    pub fn declared(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            directory: std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from),
        }
    }

    /// For a host that places credentials somewhere it names itself.
    pub fn at(name: impl Into<String>, directory: impl Into<std::path::PathBuf>) -> Self {
        Self { name: name.into(), directory: Some(directory.into()) }
    }
}

impl KeySource for InitCredentialSource {
    fn declared(&self) -> DeclaredSource {
        DeclaredSource::InitCredential
    }

    fn unwrap_key(&self) -> Result<KeyMaterial, SourceError> {
        let Some(directory) = &self.directory else {
            return Err(SourceError::Unavailable {
                source: DeclaredSource::InitCredential,
                detail: "the init system named no credentials directory",
            });
        };

        // Checked before the join: a separator would read a file outside the prepared directory.
        if self.name.is_empty() || self.name.contains('/') || self.name.contains("..") {
            return Err(SourceError::Unavailable {
                source: DeclaredSource::InitCredential,
                detail: "the credential name is not a plain file name",
            });
        }

        let bytes = std::fs::read(directory.join(&self.name))
            .map_err(|_| SourceError::Failed { source: DeclaredSource::InitCredential })?;

        // Verbatim, never stretched: a credential of the wrong length was not produced by this deployment,
        // and deriving 32 bytes from it would open the keystore under a key it was not sealed with.
        let material: [u8; 32] =
            bytes.as_slice().try_into().map_err(|_| SourceError::BadParameters)?;
        Ok(KeyMaterial::from_bytes(material))
    }
}
