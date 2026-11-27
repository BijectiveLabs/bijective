// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operator registry file: who may sign commands, with which role and which public key.
//!
//! Version 1 is a list of `{ name, role, public_key }`, the key as 64 hexadecimal characters.
//! A refusal names the entry, so an operator edits the right line.

use bijective_authority::Role;
use bijective_identity::{Operator, OperatorRegistry, from_hex_32};
use serde::Deserialize;

const OPERATORS_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct File {
    version: u32,
    operators: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    role: String,
    public_key: String,
}

/// Checks every entry before any is trusted. The error is for a person, not a program.
pub fn parse_operators(raw: &[u8]) -> Result<OperatorRegistry, String> {
    let file: File =
        serde_json::from_slice(raw).map_err(|e| format!("operators file unreadable: {e}"))?;
    if file.version != OPERATORS_VERSION {
        return Err(format!(
            "operators file version {} is not version {OPERATORS_VERSION}",
            file.version
        ));
    }
    let mut entries = Vec::with_capacity(file.operators.len());
    for entry in file.operators {
        let role = Role::named(&entry.role).ok_or_else(|| {
            format!("{}: {} is not a role this build knows", entry.name, entry.role)
        })?;
        let public_key = from_hex_32(&entry.public_key)
            .map_err(|e| format!("{}: the public key is not usable: {e}", entry.name))?;
        entries.push((entry.name, Operator { role, public_key }));
    }
    OperatorRegistry::new(entries).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "../tests/unit/operators_tests.rs"]
mod tests;
