// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Seal a keystore under the key the init system will hand the service.
//! The service opens its keystore with the init credential. Sealing under anything else fails only at start-up on the operator's node.
//! Takes `name=value` pairs after the keystore path. Operator tool, never a service.

use bijective_secrets_broker::{
    DeclaredSource, InitCredentialSource, KeySource, keystore::Keystore,
};

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(out), Some(credential)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: seal_under_init_credential <keystore-path> <credential-name> name=value..."
        );
        return std::process::ExitCode::from(2);
    };

    let key = match InitCredentialSource::declared(&credential).unwrap_key() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("the init credential could not be resolved: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut store = Keystore::new();
    for pair in args {
        let Some((name, value)) = pair.split_once('=') else {
            eprintln!("{pair} is not name=value");
            return std::process::ExitCode::from(2);
        };
        store.put(name, value.as_bytes().to_vec());
    }

    // The salt is recorded though this source does not use it: a header that varied by source would force readers to branch.
    let salt = [0u8; 16];
    match store.seal(&key, DeclaredSource::InitCredential, &salt) {
        Ok(bytes) => match std::fs::write(&out, bytes) {
            Ok(()) => {
                println!("sealed {} entr(ies) into {out}", store.len());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("could not write {out}: {e}");
                std::process::ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("could not seal: {e:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
