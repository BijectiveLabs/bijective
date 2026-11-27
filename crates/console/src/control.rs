// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the Control page offers, as a document an application of this origin reads to offer the
//! same commands through the same `/control` form.
//!
//! It gives an application nothing a page does not have: the page carries the same token in its
//! forms. A browser does not let another site read this answer, the form check refuses a post
//! from another origin, and the control engine verifies signature and role on arrival.

use std::collections::BTreeSet;

use serde_json::json;

use crate::desk::Desk;

/// The document's version. A change that breaks a reader raises it.
pub const CONTROL_FORMAT: u64 = 1;

/// The offer as JSON: `issuing` is false, with nothing else to act on, when this console was given
/// no operator.
#[must_use]
pub fn offer(desk: Option<&Desk>, mode: &str, venues: &BTreeSet<String>) -> String {
    let Some(desk) = desk else {
        return json!({ "format": CONTROL_FORMAT, "issuing": false, "mode": mode }).to_string();
    };
    let actions: Vec<&str> = desk.offers().iter().map(|a| a.as_str()).collect();
    json!({
        "format": CONTROL_FORMAT,
        "issuing": true,
        "mode": mode,
        "operator": desk.operator,
        "role": desk.role.as_str(),
        "actions": actions,
        "venues": venues,
        "token": desk.token(),
    })
    .to_string()
}

#[cfg(test)]
#[path = "../tests/unit/control_tests.rs"]
mod tests;
