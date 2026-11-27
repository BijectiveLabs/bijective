// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Identifier and transformation constraints, which bind different things.

use serde::{Deserialize, Serialize};

/// Venue assurance: some venues reject hyphens in client order IDs, which most id formats contain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifierRules {
    /// Characters permitted. An allow-list, because a deny-list is a list someone forgets.
    pub allowed: String,
    pub max_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierRejection {
    TooLong { len: usize, max: usize },
    Character { at: usize, found: char },
    Empty,
}

impl core::fmt::Display for IdentifierRejection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IdentifierRejection::TooLong { len, max } => {
                write!(f, "identifier is {len} characters, venue accepts {max}")
            }
            IdentifierRejection::Character { at, found } => {
                write!(f, "character {found:?} at position {at} is not accepted by this venue")
            }
            IdentifierRejection::Empty => write!(f, "identifier is empty"),
        }
    }
}

impl std::error::Error for IdentifierRejection {}

impl IdentifierRules {
    /// Rules are data so the check moves with the venue.
    pub fn check(&self, candidate: &str) -> Result<(), IdentifierRejection> {
        if candidate.is_empty() {
            return Err(IdentifierRejection::Empty);
        }
        if candidate.chars().count() > self.max_len {
            return Err(IdentifierRejection::TooLong {
                len: candidate.chars().count(),
                max: self.max_len,
            });
        }
        for (at, ch) in candidate.chars().enumerate() {
            if !self.allowed.contains(ch) {
                return Err(IdentifierRejection::Character { at, found: ch });
            }
        }
        Ok(())
    }
}

/// Venue assurance: a value altered in flight is not the value we asked for, so a benchmark assuming
/// verbatim transmission measures a fiction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Transformation {
    PriceSignificantFigures { field: String, figures: u8 },
    IdentifierNarrowed { field: String, to_bits: u16 },
}

impl Transformation {
    pub fn field(&self) -> &str {
        match self {
            Transformation::PriceSignificantFigures { field, .. }
            | Transformation::IdentifierNarrowed { field, .. } => field,
        }
    }
}

impl core::fmt::Display for Transformation {
    /// Shows what a value was narrowed to, which says whether the narrowing is tolerable.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Transformation::PriceSignificantFigures { field, figures } => {
                write!(f, "{field} was cut to {figures} significant figure(s)")
            }
            Transformation::IdentifierNarrowed { field, to_bits } => {
                write!(f, "{field} was narrowed to {to_bits} bits")
            }
        }
    }
}
