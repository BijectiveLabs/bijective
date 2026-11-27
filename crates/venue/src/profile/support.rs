// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! V6 - which venues are supported, and the hard rule made structural.

use serde::{Deserialize, Serialize};

/// Depth is declared, not inferred from whether an adapter compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportDepth {
    /// Market data and execution.
    Full,
    /// Market data only. No order may be placed here.
    DataOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Wire")]
pub struct SupportEntry {
    pub venue: String,
    pub depth: SupportDepth,
    /// The hard rule's second half. `declare` and the parse both refuse `false`.
    debuggable_to_root_cause: bool,
}

/// Parsed shape before the rule applies: a derived `Deserialize` would skip it for entries
/// read from a file.
#[derive(Deserialize)]
struct Wire {
    venue: String,
    depth: SupportDepth,
    debuggable_to_root_cause: bool,
}

impl TryFrom<Wire> for SupportEntry {
    type Error = SupportError;

    fn try_from(w: Wire) -> Result<Self, Self::Error> {
        SupportEntry::declare(&w.venue, w.depth, w.debuggable_to_root_cause)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportError {
    /// Refused at declaration and at parse: support for a venue we cannot debug to root cause is a
    /// commitment we cannot keep.
    NotDebuggable { venue: String },
}

impl core::fmt::Display for SupportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SupportError::NotDebuggable { venue } => write!(
                f,
                "venue {venue} cannot be declared: we support only venues we can debug to root cause"
            ),
        }
    }
}

impl std::error::Error for SupportError {}

impl SupportEntry {
    /// The hard rule is enforced here, not reviewed: such an entry cannot be constructed by
    /// any path.
    pub fn declare(
        venue: &str,
        depth: SupportDepth,
        debuggable_to_root_cause: bool,
    ) -> Result<Self, SupportError> {
        if !debuggable_to_root_cause {
            return Err(SupportError::NotDebuggable { venue: venue.to_string() });
        }
        Ok(Self { venue: venue.to_string(), depth, debuggable_to_root_cause })
    }

    pub fn depth(&self) -> SupportDepth {
        self.depth
    }
}
