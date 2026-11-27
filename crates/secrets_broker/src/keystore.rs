// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The encrypted keystore: format, sealing, and the atomic rewrite rotation depends on.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::credential::{Credential, KeyMaterial};
use crate::source::DeclaredSource;

/// Header magic. Distinguishes a keystore from anything else on disk before a key is applied.
pub const KEYSTORE_MAGIC: [u8; 4] = *b"BJKS";

/// On-disk format version, checked on open. A future version is refused, not parsed on assumption.
pub const KEYSTORE_VERSION: u16 = 1;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 4 + 2 + 1 + SALT_LEN + NONCE_LEN;

/// A field longer than this is refused before anything is allocated.
const MAX_FIELD_LEN: u32 = 64 * 1024;

#[derive(Debug)]
pub enum KeystoreError {
    NotAKeystore,
    UnsupportedVersion {
        found: u16,
    },
    /// The declared source in the file is not the one the caller resolved with.
    /// Refused: a keystore sealed under a TPM and opened with a passphrase means the deployment is not what it declares.
    SourceMismatch {
        in_file: u8,
        resolved: DeclaredSource,
    },
    /// Authentication failed: wrong key or altered file. One error for both, since distinguishing them helps an attacker.
    Unopenable,
    Malformed,
    Io(std::io::Error),
}

impl From<std::io::Error> for KeystoreError {
    fn from(e: std::io::Error) -> Self {
        KeystoreError::Io(e)
    }
}

impl std::fmt::Display for KeystoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeystoreError::NotAKeystore => write!(f, "not a keystore"),
            KeystoreError::UnsupportedVersion { found } => {
                write!(f, "keystore format version {found} is newer than this build")
            }
            KeystoreError::SourceMismatch { in_file, resolved } => write!(
                f,
                "keystore was sealed under source {in_file}, opened with {}",
                resolved.as_str()
            ),
            KeystoreError::Unopenable => write!(f, "keystore could not be opened"),
            KeystoreError::Malformed => write!(f, "keystore contents are malformed"),
            KeystoreError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for KeystoreError {}

fn source_code(s: DeclaredSource) -> u8 {
    match s {
        DeclaredSource::InitCredential => 0,
        DeclaredSource::TpmSeal => 1,
        DeclaredSource::Passphrase => 2,
    }
}

/// The salt a passphrase source needs, read without opening the store.
pub fn read_salt(bytes: &[u8]) -> Result<Vec<u8>, KeystoreError> {
    if bytes.len() < HEADER_LEN || bytes[0..4] != KEYSTORE_MAGIC {
        return Err(KeystoreError::NotAKeystore);
    }
    Ok(bytes[7..7 + SALT_LEN].to_vec())
}

/// Named credentials, held only while the process needs them.
#[derive(Debug, Default)]
pub struct Keystore {
    entries: BTreeMap<String, Credential>,
}

impl Keystore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, name: &str, secret: Vec<u8>) {
        self.entries.insert(name.to_string(), Credential::new(secret));
    }

    /// Resolve by name. The name is not the secret, so it may appear in an error.
    pub fn get(&self, name: &str) -> Option<&Credential> {
        self.entries.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Seal into bytes under `key`, recording which source was declared.
    /// A fresh nonce per seal, generated not derived: reuse under one key breaks the construction completely.
    pub fn seal(
        &self,
        key: &KeyMaterial,
        source: DeclaredSource,
        salt: &[u8],
    ) -> Result<Vec<u8>, KeystoreError> {
        if salt.len() != SALT_LEN {
            return Err(KeystoreError::Malformed);
        }
        let mut plain = Vec::new();
        for (name, cred) in &self.entries {
            let n = name.as_bytes();
            let s = cred.expose();
            if n.len() as u32 > MAX_FIELD_LEN || s.len() as u32 > MAX_FIELD_LEN {
                return Err(KeystoreError::Malformed);
            }
            plain.extend_from_slice(&(n.len() as u32).to_be_bytes());
            plain.extend_from_slice(n);
            plain.extend_from_slice(&(s.len() as u32).to_be_bytes());
            plain.extend_from_slice(s);
        }

        let cipher = ChaCha20Poly1305::new(&Key::from(*key.expose()));
        let nonce = Nonce::generate();
        let sealed =
            cipher.encrypt(&nonce, plain.as_ref()).map_err(|_| KeystoreError::Unopenable)?;

        let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
        out.extend_from_slice(&KEYSTORE_MAGIC);
        out.extend_from_slice(&KEYSTORE_VERSION.to_be_bytes());
        out.push(source_code(source));
        out.extend_from_slice(salt);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    pub fn open(
        bytes: &[u8],
        key: &KeyMaterial,
        resolved: DeclaredSource,
    ) -> Result<Self, KeystoreError> {
        if bytes.len() < HEADER_LEN || bytes[0..4] != KEYSTORE_MAGIC {
            return Err(KeystoreError::NotAKeystore);
        }
        let version = u16::from_be_bytes([bytes[4], bytes[5]]);
        if version != KEYSTORE_VERSION {
            return Err(KeystoreError::UnsupportedVersion { found: version });
        }
        let in_file = bytes[6];
        if in_file != source_code(resolved) {
            return Err(KeystoreError::SourceMismatch { in_file, resolved });
        }

        let nonce_at = 7 + SALT_LEN;
        let nonce = Nonce::try_from(&bytes[nonce_at..nonce_at + NONCE_LEN])
            .map_err(|_| KeystoreError::Malformed)?;
        let cipher = ChaCha20Poly1305::new(&Key::from(*key.expose()));
        let plain =
            cipher.decrypt(&nonce, &bytes[HEADER_LEN..]).map_err(|_| KeystoreError::Unopenable)?;

        let mut store = Self::new();
        let mut at = 0usize;
        while at < plain.len() {
            let name = take_field(&plain, &mut at)?;
            let secret = take_field(&plain, &mut at)?;
            let name = String::from_utf8(name).map_err(|_| KeystoreError::Malformed)?;
            store.entries.insert(name, Credential::new(secret));
        }
        Ok(store)
    }

    /// Write to `path` atomically, so an interrupted rotation leaves the previous keystore.
    /// A keystore is state, not a log: the safe failure is unchanged, the opposite of the event store.
    pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), KeystoreError> {
        let tmp = path.with_extension("tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        restrict(path)?;
        Ok(())
    }
}

fn take_field(buf: &[u8], at: &mut usize) -> Result<Vec<u8>, KeystoreError> {
    if buf.len() - *at < 4 {
        return Err(KeystoreError::Malformed);
    }
    let len = u32::from_be_bytes([buf[*at], buf[*at + 1], buf[*at + 2], buf[*at + 3]]);
    if len > MAX_FIELD_LEN {
        return Err(KeystoreError::Malformed);
    }
    *at += 4;
    let len = len as usize;
    if buf.len() - *at < len {
        return Err(KeystoreError::Malformed);
    }
    let out = buf[*at..*at + len].to_vec();
    *at += len;
    Ok(out)
}

/// Owner-only permissions on the keystore.
/// An explicit mode, not `set_readonly(false)`, which makes a file world writable on Unix.
#[cfg(unix)]
fn restrict(path: &Path) -> Result<(), KeystoreError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<(), KeystoreError> {
    Ok(())
}
