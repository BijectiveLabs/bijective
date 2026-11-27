// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! How long to wait before the next attempt: exponential, bounded, and spread.

use super::jitter::Jitter;

/// A wait that grows with consecutive failures up to a ceiling, then is spread over its upper half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    base_ns: i128,
    ceiling_ns: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffError {
    BaseNotPositive,
    CeilingBelowBase,
}

impl std::fmt::Display for BackoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BaseNotPositive => write!(f, "a backoff base must be above zero"),
            Self::CeilingBelowBase => write!(f, "a backoff ceiling must not be below its base"),
        }
    }
}

impl std::error::Error for BackoffError {}

impl Backoff {
    pub const fn new(base_ns: i128, ceiling_ns: i128) -> Result<Self, BackoffError> {
        if base_ns <= 0 {
            return Err(BackoffError::BaseNotPositive);
        }
        if ceiling_ns < base_ns {
            return Err(BackoffError::CeilingBelowBase);
        }
        Ok(Self { base_ns, ceiling_ns })
    }

    /// The longest wait after `failures` consecutive failures: the base doubled once per failure,
    /// never above the ceiling. Safe for any count. A reconnect loop can run for days.
    #[must_use]
    pub fn longest_ns(self, failures: u32) -> i128 {
        let doubled = self.base_ns.saturating_mul(1i128 << failures.min(100));
        doubled.min(self.ceiling_ns)
    }

    /// The wait to use: between half of the longest and all of it. The lower half is kept so that
    /// a retry always backs off by something, and the upper half spreads clients apart.
    #[must_use]
    pub fn delay_ns(self, failures: u32, jitter: &mut dyn Jitter) -> i128 {
        let longest = self.longest_ns(failures);
        let floor = longest / 2;
        let span = u64::try_from(longest - floor + 1).unwrap_or(u64::MAX);
        floor + i128::from(jitter.below(span))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resilience/backoff_tests.rs"]
mod tests;
