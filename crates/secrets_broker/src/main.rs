// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operator's path to the keystore, not one of the process model's six long-lived programs.
//! No secret comes from an argument (visible in `ps`, kept in history). Stdin only. Runbook 07.
//! A keystore is sealed under a passphrase, or with `--init-credential <file>` under the 32-byte
//! init credential the node's units load: the only seal the engines open.

use std::io::{BufRead, Read, Write};
use std::path::Path;

use bijective_secrets_broker::keystore::{Keystore, read_salt};
use bijective_secrets_broker::{
    DeclaredSource, InitCredentialSource, KeyMaterial, KeySource, PassphraseSource,
};

const SALT_LEN: usize = 16;
const INIT_FLAG: &str = "--init-credential";

/// A key, the source it is declared under, and the salt it is sealed with.
type Sealing = (KeyMaterial, DeclaredSource, Vec<u8>);

/// Which key seals the keystore. The init credential is named by its file, read verbatim.
enum Seal {
    Passphrase,
    InitCredential(InitCredentialSource),
}

fn main() -> std::process::ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let seal = match take_seal(&mut args) {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let (command, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => return usage(),
    };
    let outcome = match (command, rest) {
        ("init", [path]) => run_init(Path::new(path), &seal),
        ("put", [path, name]) => run_put(Path::new(path), name, &seal),
        ("list", [path]) => run_list(Path::new(path), &seal),
        _ => return usage(),
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// Removes `--init-credential <file>` from the arguments, wherever it stands.
fn take_seal(args: &mut Vec<String>) -> Result<Seal, String> {
    let Some(at) = args.iter().position(|a| a == INIT_FLAG) else {
        return Ok(Seal::Passphrase);
    };
    if at + 1 >= args.len() {
        return Err(format!("{INIT_FLAG} wants the credential's file"));
    }
    let file = std::path::PathBuf::from(args.remove(at + 1));
    args.remove(at);
    let (Some(directory), Some(name)) = (file.parent(), file.file_name()) else {
        return Err(format!("{} names no credential file", file.display()));
    };
    let name = name.to_string_lossy().into_owned();
    Ok(Seal::InitCredential(InitCredentialSource::at(name, directory)))
}

fn usage() -> std::process::ExitCode {
    eprintln!(
        "usage: bijective-secrets [{INIT_FLAG} <file>] <init|put|list> <keystore-path> [name]"
    );
    eprintln!();
    eprintln!("Secrets are read from standard input, never from an argument.");
    eprintln!("  init  reads: passphrase (none with {INIT_FLAG})");
    eprintln!("  put   reads: passphrase (none with {INIT_FLAG}), then the secret, one per line");
    eprintln!("  list  reads: passphrase (none with {INIT_FLAG})");
    std::process::ExitCode::from(2)
}

/// Read one line, and refuse an empty one.
/// A blank passphrase or secret is almost always an empty pipeline, and sealing under it would look like it worked.
fn read_line(stdin: &mut impl BufRead, what: &str) -> Result<Vec<u8>, String> {
    let mut line = String::new();
    stdin.read_line(&mut line).map_err(|e| format!("could not read {what}: {e}"))?;
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(format!("{what} is empty"));
    }
    Ok(trimmed.as_bytes().to_vec())
}

/// Sixteen bytes from the operating system's own generator.
/// `/dev/urandom` rather than a crate: a salt only has to be unique, not secret.
fn fresh_salt() -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|e| format!("no system randomness available: {e}"))?;
    let mut salt = vec![0u8; SALT_LEN];
    file.read_exact(&mut salt).map_err(|e| format!("could not read randomness: {e}"))?;
    Ok(salt)
}

/// The key, its declared source and the salt it is sealed with. A passphrase is read here. The
/// init credential's salt is recorded unused, as the engines' keystores record it.
fn key_for(
    seal: &Seal,
    salt: Option<Vec<u8>>,
    stdin: &mut impl BufRead,
) -> Result<Sealing, String> {
    match seal {
        Seal::Passphrase => {
            let passphrase = read_line(stdin, "passphrase")?;
            let salt = match salt {
                Some(s) => s,
                None => fresh_salt()?,
            };
            let key = PassphraseSource::new(passphrase, salt.clone())
                .unwrap_key()
                .map_err(|e| format!("could not derive the keystore key: {e}"))?;
            Ok((key, DeclaredSource::Passphrase, salt))
        }
        Seal::InitCredential(source) => {
            let key = source
                .unwrap_key()
                .map_err(|e| format!("could not read the init credential: {e}"))?;
            Ok((key, DeclaredSource::InitCredential, vec![0u8; SALT_LEN]))
        }
    }
}

fn seal_to(path: &Path, keystore: &Keystore, key: &Sealing) -> Result<(), String> {
    let (material, source, salt) = key;
    let sealed = keystore
        .seal(material, *source, salt)
        .map_err(|e| format!("could not seal the keystore: {e:?}"))?;
    Keystore::write_atomically(path, &sealed).map_err(|e| format!("could not write: {e:?}"))
}

fn open_at(
    path: &Path,
    seal: &Seal,
    stdin: &mut impl BufRead,
) -> Result<(Keystore, Sealing), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read the keystore: {e}"))?;
    let salt = read_salt(&bytes).map_err(|e| format!("not a keystore: {e:?}"))?;
    let key = key_for(seal, Some(salt), stdin)?;
    let keystore = Keystore::open(&bytes, &key.0, key.1)
        .map_err(|e| format!("could not open the keystore: {e:?}"))?;
    Ok((keystore, key))
}

fn run_init(path: &Path, seal: &Seal) -> Result<(), String> {
    // Refused rather than overwritten: `init` over a populated keystore would discard every credential.
    if path.exists() {
        return Err(format!("{} already exists; refusing to replace it", path.display()));
    }
    let stdin = std::io::stdin();
    let key = key_for(seal, None, &mut stdin.lock())?;
    seal_to(path, &Keystore::new(), &key)?;
    println!("created an empty keystore at {}", path.display());
    Ok(())
}

fn run_put(path: &Path, name: &str, seal: &Seal) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a credential must have a name".into());
    }
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    let (mut keystore, key) = open_at(path, seal, &mut locked)?;
    let secret = read_line(&mut locked, "secret")?;
    let replaced = keystore.get(name).is_some();
    keystore.put(name, secret);
    seal_to(path, &keystore, &key)?;
    // The name, never the value: an echoed secret lands in a scrollback buffer.
    println!("{} {name}", if replaced { "replaced" } else { "stored" });
    Ok(())
}

fn run_list(path: &Path, seal: &Seal) -> Result<(), String> {
    let stdin = std::io::stdin();
    let (keystore, _) = open_at(path, seal, &mut stdin.lock())?;
    for name in keystore.names() {
        println!("{name}");
    }
    Ok(())
}

fn fail(why: &str) -> std::process::ExitCode {
    let _ = writeln!(std::io::stderr(), "bijective-secrets: {why}");
    std::process::ExitCode::FAILURE
}
