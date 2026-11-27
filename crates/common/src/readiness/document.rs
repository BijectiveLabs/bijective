// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a program says about itself, and the one question it answers.

use serde::{Deserialize, Serialize};

/// The readiness document's own version. An unknown version is refused, not a best effort.
pub const DOCUMENT_VERSION: u32 = 1;

/// How stale a document may be before a reader stops believing it.
/// A dead program leaves its last claim behind saying `Ready`. Without a bound the file reports a dead node as healthy.
pub const MAX_AGE_NS: i128 = 15_000_000_000;

/// Where a program is in its own lifecycle - and nothing else.
/// Absent on purpose: `Running` (running is not ready, the deployment kit) and the trading state, a different question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Starting,
    /// Replaying durable state. The order path is not open and must not be.
    Replaying,
    Ready,
    Failed,
}

impl Health {
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    pub version: u32,
    pub program: String,
    pub health: Health,
    /// Why, in the program's own words. Required even when healthy.
    pub detail: String,
    pub stamped_at_ns: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadinessError {
    Absent,
    Unreadable,
    VersionUnknown { found: u32 },
    Stale { age_ns: i128 },
}

impl core::fmt::Display for ReadinessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Absent => write!(f, "no readiness document, so the program has not reported"),
            Self::Unreadable => write!(f, "the readiness document could not be read"),
            Self::VersionUnknown { found } => {
                write!(f, "readiness document version {found} is not version {DOCUMENT_VERSION}")
            }
            Self::Stale { age_ns } => {
                write!(f, "the last report is {}s old", age_ns / 1_000_000_000)
            }
        }
    }
}

impl std::error::Error for ReadinessError {}

impl Readiness {
    pub fn new(
        program: impl Into<String>,
        health: Health,
        detail: impl Into<String>,
        at_ns: i128,
    ) -> Self {
        Self {
            version: DOCUMENT_VERSION,
            program: program.into(),
            health,
            detail: detail.into(),
            stamped_at_ns: at_ns.to_string(),
        }
    }

    /// Wide integers travel as strings because JSON numbers are IEEE-754 doubles and a
    /// nanosecond timestamp does not survive one.
    pub fn stamped_at_ns(&self) -> Option<i128> {
        self.stamped_at_ns.parse().ok()
    }
}
