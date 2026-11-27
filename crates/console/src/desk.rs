// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The control desk: the one operator this console signs commands as, and the form check that
//! stands between a browser and a signed command.
//!
//! The console decides nothing. It signs what the operator asked for with the operator's own key,
//! appends it to the `commands` stream, and the control engine verifies the signature and the
//! role exactly as it does for the command line. What came of a command is read back from the
//! control stream, like everything else on the page.

use std::path::{Path, PathBuf};

use bijective_authority::{Action, Role, decide};
use bijective_cli::{issue, load_key, parse_instruction};
use bijective_identity::to_hex;
use bijective_serialization::signal::node::NodeStreams;

/// The actions the control page offers. Position adjustments stay on the command line: they carry
/// quantities and prices a form would invite mistyping.
const OFFERED: [Action; 7] = [
    Action::Halt,
    Action::Reduce,
    Action::Resume,
    Action::CancelOrders,
    Action::Flatten,
    Action::WithdrawVenue,
    Action::AdmitVenue,
];

/// A submitted form is a handful of short fields. Anything longer is not one of ours.
pub const MAX_FORM_BYTES: usize = 1024;

pub struct Desk {
    pub operator: String,
    pub role: Role,
    key_file: PathBuf,
    /// Carried by every form this console renders and required on every submission, so a page
    /// on another site cannot make the operator's browser submit one.
    token: String,
}

/// Why a submission was not signed. Nothing reached the command stream in any of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The form did not carry this console's token.
    NotThisConsole,
    /// The action is not one the page offers, or the operator's role may not take it.
    NotOffered {
        action: String,
    },
    Malformed {
        detail: String,
    },
    /// Signing or appending failed. The detail names the file or stream.
    NotIssued {
        detail: String,
    },
}

impl Desk {
    /// Precondition: `token_bytes` come from a cryptographic source. The key is loaded once here
    /// so a wrong path or a damaged key stops the console at start, and is read again per command.
    pub fn new(
        operator: String,
        role: Role,
        key_file: PathBuf,
        token_bytes: &[u8; 32],
    ) -> Result<Self, String> {
        load_key(&key_file)?;
        Ok(Self { operator, role, key_file, token: to_hex(token_bytes) })
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The offered actions this operator's role may take.
    #[must_use]
    pub fn offers(&self) -> Vec<Action> {
        OFFERED.into_iter().filter(|a| decide(self.role, *a).is_permitted()).collect()
    }

    /// Signs and appends the command a form asks for. Postcondition: `Ok` is the command stream
    /// position it landed at. Every `Err` left the stream untouched.
    pub fn submit(&self, store_root: &Path, node: NodeStreams, form: &str) -> Result<u64, Refusal> {
        if form.len() > MAX_FORM_BYTES {
            return Err(Refusal::Malformed {
                detail: "the form is longer than any of ours".into(),
            });
        }
        let fields = form_fields(form)?;
        let field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        if !same_token(field("token").as_deref().unwrap_or(""), &self.token) {
            return Err(Refusal::NotThisConsole);
        }
        let action = field("action").unwrap_or_default();
        if !self.offers().iter().any(|a| a.as_str() == action) {
            return Err(Refusal::NotOffered { action });
        }
        let venue = field("venue").filter(|v| !v.is_empty());
        let instruction = parse_instruction(&action, venue, &[])
            .map_err(|detail| Refusal::Malformed { detail })?;
        // For the node whose record this console reads, never another: a paper console's halt
        // halts the paper node.
        let signer = (self.operator.as_str(), self.role, self.key_file.as_path());
        issue(store_root, signer, instruction, node).map_err(|detail| Refusal::NotIssued { detail })
    }
}

/// Compared in full whatever the first difference, so the time taken says nothing about the token.
fn same_token(given: &str, held: &str) -> bool {
    let (given, held) = (given.as_bytes(), held.as_bytes());
    let differ = given.iter().zip(held).fold(0_u8, |acc, (a, b)| acc | (a ^ b));
    given.len() == held.len() && differ == 0
}

/// `application/x-www-form-urlencoded` pairs. Only the characters our own fields can hold are
/// accepted after decoding, so a value cannot carry markup or a path.
fn form_fields(form: &str) -> Result<Vec<(String, String)>, Refusal> {
    form.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((decoded(name)?, decoded(value)?))
        })
        .collect()
}

fn decoded(raw: &str) -> Result<String, Refusal> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = raw.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok());
                let Some(byte) = hex else {
                    return Err(Refusal::Malformed { detail: "a broken percent escape".into() });
                };
                out.push(byte);
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    let text = String::from_utf8(out)
        .map_err(|_| Refusal::Malformed { detail: "a field that is not text".into() })?;
    if text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        Ok(text)
    } else {
        Err(Refusal::Malformed { detail: "a field holds a character no field of ours has".into() })
    }
}

#[cfg(test)]
#[path = "../tests/unit/desk_tests.rs"]
mod tests;
