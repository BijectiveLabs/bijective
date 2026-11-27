// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A resolution is a person's decision, recorded.

use crate::classify::Divergence;

/// The operator owns the decision. The constructor is the only way in, so attribution cannot be skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    divergence: Divergence,
    resolved_by: String,
    reason: String,
    resolved_at_ns: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionError {
    /// Reconciliation: an unattributed resolution is worse than no record.
    NoPersonNamed,
    NoReasonGiven,
}

impl core::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPersonNamed => write!(f, "a resolution must name the person who decided"),
            Self::NoReasonGiven => write!(f, "a resolution must carry the reason"),
        }
    }
}

impl std::error::Error for ResolutionError {}

impl Resolution {
    pub fn record(
        divergence: Divergence,
        resolved_by: &str,
        reason: &str,
        resolved_at_ns: i128,
    ) -> Result<Self, ResolutionError> {
        if resolved_by.trim().is_empty() {
            return Err(ResolutionError::NoPersonNamed);
        }
        if reason.trim().is_empty() {
            return Err(ResolutionError::NoReasonGiven);
        }
        Ok(Self {
            divergence,
            resolved_by: resolved_by.trim().to_string(),
            reason: reason.trim().to_string(),
            resolved_at_ns,
        })
    }

    pub fn divergence(&self) -> &Divergence {
        &self.divergence
    }

    pub fn resolved_by(&self) -> &str {
        &self.resolved_by
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn resolved_at_ns(&self) -> i128 {
        self.resolved_at_ns
    }
}
