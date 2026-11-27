// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Gates for the broker. Each is a way a credential could escape or a control could weaken.

use super::*;
use crate::source::KeySource;

fn key() -> KeyMaterial {
    KeyMaterial::from_bytes([7u8; 32])
}

const SALT: &[u8] = b"sixteen-byte-slt";

#[test]
fn a_credential_has_no_rendering_that_reveals_it() {
    let c = Credential::new(b"venue-api-secret".to_vec());
    let shown = format!("{c:?}");
    assert!(!shown.contains("venue"), "debug leaked the value: {shown}");
    assert!(shown.contains("redacted"));
    assert_eq!(c.expose(), b"venue-api-secret");
}

#[test]
fn key_material_does_not_render_either() {
    let shown = format!("{:?}", key());
    assert_eq!(shown, "KeyMaterial(redacted)");
}

#[test]
fn a_keystore_round_trips() {
    let mut ks = Keystore::new();
    ks.put("venue-a/execution", b"abc123".to_vec());
    ks.put("venue-a/data", b"read-only".to_vec());

    let sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    let back = Keystore::open(&sealed, &key(), DeclaredSource::Passphrase).unwrap();

    assert_eq!(back.len(), 2);
    assert_eq!(back.get("venue-a/execution").unwrap().expose(), b"abc123");
    assert_eq!(back.get("venue-a/data").unwrap().expose(), b"read-only");
}

/// The sealed bytes must not contain the secret.
#[test]
fn the_sealed_form_does_not_contain_the_plaintext() {
    let mut ks = Keystore::new();
    ks.put("k", b"UNIQUE-MARKER-VALUE".to_vec());
    let sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    assert!(!sealed.windows(19).any(|w| w == b"UNIQUE-MARKER-VALUE"), "plaintext survived sealing");
}

/// Two seals of the same content differ, because the nonce is fresh each time.
/// Nonce reuse under one key breaks the construction completely, so it is asserted.
#[test]
fn two_seals_of_the_same_store_differ() {
    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let a = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    let b = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    assert_ne!(a, b, "the nonce was not fresh");
}

#[test]
fn a_wrong_key_is_refused_and_says_nothing_about_why() {
    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();

    let wrong = KeyMaterial::from_bytes([9u8; 32]);
    let err = Keystore::open(&sealed, &wrong, DeclaredSource::Passphrase).unwrap_err();
    assert!(matches!(err, KeystoreError::Unopenable));
}

/// A single altered byte fails authentication. The AEAD tag is what makes tampering visible.
#[test]
fn a_tampered_keystore_is_refused() {
    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let mut sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;
    assert!(matches!(
        Keystore::open(&sealed, &key(), DeclaredSource::Passphrase),
        Err(KeystoreError::Unopenable)
    ));
}

/// Opening under a source other than the one it was sealed with is refused.
/// Continuing would hide that the deployment is not what it declares.
#[test]
fn a_source_mismatch_is_refused() {
    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let sealed = ks.seal(&key(), DeclaredSource::TpmSeal, SALT).unwrap();
    assert!(matches!(
        Keystore::open(&sealed, &key(), DeclaredSource::Passphrase),
        Err(KeystoreError::SourceMismatch { .. })
    ));
}

#[test]
fn a_newer_format_version_is_refused() {
    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let mut sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    sealed[4..6].copy_from_slice(&(keystore::KEYSTORE_VERSION + 1).to_be_bytes());
    assert!(matches!(
        Keystore::open(&sealed, &key(), DeclaredSource::Passphrase),
        Err(KeystoreError::UnsupportedVersion { .. })
    ));
}

#[test]
fn something_that_is_not_a_keystore_is_refused_before_a_key_is_applied() {
    assert!(matches!(
        Keystore::open(b"not ours at all", &key(), DeclaredSource::Passphrase),
        Err(KeystoreError::NotAKeystore)
    ));
}

/// There is no fallback path in the type, so this test asserts a shape rather than a behaviour.
#[test]
fn a_declared_source_names_itself_and_offers_no_ordering() {
    for s in [DeclaredSource::InitCredential, DeclaredSource::TpmSeal, DeclaredSource::Passphrase] {
        assert!(!s.as_str().is_empty());
    }
    // Absent by design: no Default, no Ord, no next, so a second source cannot be tried after a failure.
    let p = PassphraseSource::new(b"correct horse".to_vec(), SALT.to_vec());
    assert_eq!(p.declared(), DeclaredSource::Passphrase);
}

#[test]
fn a_passphrase_derives_a_key_and_is_stable() {
    let a = PassphraseSource::new(b"correct horse".to_vec(), SALT.to_vec()).unwrap_key().unwrap();
    let b = PassphraseSource::new(b"correct horse".to_vec(), SALT.to_vec()).unwrap_key().unwrap();
    assert_eq!(a.expose(), b.expose());

    let c = PassphraseSource::new(b"wrong horse".to_vec(), SALT.to_vec()).unwrap_key().unwrap();
    assert_ne!(a.expose(), c.expose());
}

#[test]
fn a_short_salt_or_empty_passphrase_is_refused() {
    assert!(matches!(
        PassphraseSource::new(b"pw".to_vec(), b"short".to_vec()).unwrap_key(),
        Err(SourceError::BadParameters)
    ));
    assert!(matches!(
        PassphraseSource::new(Vec::new(), SALT.to_vec()).unwrap_key(),
        Err(SourceError::BadParameters)
    ));
}

#[test]
fn the_salt_is_readable_without_opening_the_store() {
    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    assert_eq!(keystore::read_salt(&sealed).unwrap(), SALT);
}

/// An interrupted rotation must leave the previous keystore intact.
#[test]
fn an_atomic_write_replaces_or_leaves_alone() {
    let dir = std::env::temp_dir().join(format!("bijective-secrets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keystore.bjks");

    let mut first = Keystore::new();
    first.put("k", b"one".to_vec());
    let a = first.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    Keystore::write_atomically(&path, &a).unwrap();

    let mut second = Keystore::new();
    second.put("k", b"two".to_vec());
    let b = second.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    Keystore::write_atomically(&path, &b).unwrap();

    let read = std::fs::read(&path).unwrap();
    let back = Keystore::open(&read, &key(), DeclaredSource::Passphrase).unwrap();
    assert_eq!(back.get("k").unwrap().expose(), b"two");
    assert!(!path.with_extension("tmp").exists(), "the temporary file was left behind");
}

/// The keystore on disk is readable by its owner only (mode 0600).
/// Guards against `set_readonly(false)`, which makes the file world writable on Unix.
#[cfg(unix)]
#[test]
fn the_keystore_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("bijective-secrets-perm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keystore.bjks");

    let mut ks = Keystore::new();
    ks.put("k", b"v".to_vec());
    let sealed = ks.seal(&key(), DeclaredSource::Passphrase, SALT).unwrap();
    Keystore::write_atomically(&path, &sealed).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "keystore mode is {mode:o}, expected 600");
}

fn credential_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-cred-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// The key is taken verbatim, because the init system generated it as key material.
#[test]
fn an_init_credential_is_used_as_it_was_delivered() {
    let dir = credential_dir("verbatim");
    let material = [7u8; 32];
    std::fs::write(dir.join("keystore-key"), material).expect("write");

    let key = InitCredentialSource::at("keystore-key", &dir).unwrap_key().expect("present");

    assert_eq!(key.expose(), &material, "a KDF here would open nothing");
}

/// A host that named no credentials directory is refused, never fallen back from.
/// The configured source is the only one tried.
#[test]
fn a_host_with_no_credentials_directory_is_refused() {
    if std::env::var_os("CREDENTIALS_DIRECTORY").is_some() {
        eprintln!("SKIPPED: this host names a credentials directory");
        return;
    }

    // Reached through `declared`, not a test-only constructor: the test must enter the way a deployment does.
    assert_eq!(
        InitCredentialSource::declared("keystore-key").unwrap_key().expect_err("no directory"),
        SourceError::Unavailable {
            source: DeclaredSource::InitCredential,
            detail: "the init system named no credentials directory",
        }
    );
}

/// A credential of the wrong length is refused rather than stretched into shape.
#[test]
fn a_credential_of_the_wrong_length_is_refused() {
    let dir = credential_dir("short");
    std::fs::write(dir.join("keystore-key"), [1u8; 16]).expect("write");

    assert_eq!(
        InitCredentialSource::at("keystore-key", &dir).unwrap_key().expect_err("wrong length"),
        SourceError::BadParameters
    );
}

/// A name that could leave the directory the init system prepared is refused.
#[test]
fn a_credential_name_that_is_not_a_plain_file_name_is_refused() {
    let dir = credential_dir("escape");

    for name in ["../outside", "sub/dir", ""] {
        assert!(
            matches!(
                InitCredentialSource::at(name, &dir).unwrap_key().err(),
                Some(SourceError::Unavailable { .. })
            ),
            "{name} should not be joined onto the credentials directory"
        );
    }
}

/// An absent credential is a failure of the declared source, named as such.
#[test]
fn an_absent_credential_names_the_source_that_failed() {
    let dir = credential_dir("absent");

    assert_eq!(
        InitCredentialSource::at("keystore-key", &dir).unwrap_key().expect_err("absent"),
        SourceError::Failed { source: DeclaredSource::InitCredential }
    );
}
