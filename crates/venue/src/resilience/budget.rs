// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! How fast to ask: a quota of requests per period with a burst, and the venue's own `Retry-After`.
//!
//! The generic cell rate algorithm: each request advances a theoretical arrival time by one
//! interval, and a request that would run further ahead of the clock than the burst allows is
//! told how long to wait. A refused request spends nothing.

/// `requests` per `per_ns`, with a burst of `requests`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub requests: u32,
    pub per_ns: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    NoRequests,
    NoPeriod,
    NoWholeInterval,
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRequests => write!(f, "a quota needs at least one request"),
            Self::NoPeriod => write!(f, "a quota needs a period above zero"),
            Self::NoWholeInterval => {
                write!(f, "a quota's period must give each request a nanosecond")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

#[derive(Debug, Clone)]
pub struct RequestBudget {
    interval_ns: i128,
    burst_tolerance_ns: i128,
    arrival_ns: i128,
    blocked_until_ns: i128,
}

impl RequestBudget {
    pub fn new(quota: Quota) -> Result<Self, BudgetError> {
        if quota.requests == 0 {
            return Err(BudgetError::NoRequests);
        }
        if quota.per_ns <= 0 {
            return Err(BudgetError::NoPeriod);
        }
        let interval_ns = quota.per_ns / i128::from(quota.requests);
        if interval_ns == 0 {
            return Err(BudgetError::NoWholeInterval);
        }
        Ok(Self {
            interval_ns,
            burst_tolerance_ns: interval_ns * i128::from(quota.requests - 1),
            arrival_ns: 0,
            blocked_until_ns: 0,
        })
    }

    /// Takes one request from the budget, or returns how many nanoseconds to wait before asking
    /// again. A venue's `Retry-After` is honoured first.
    pub fn acquire(&mut self, now_ns: i128) -> Result<(), i128> {
        if now_ns < self.blocked_until_ns {
            return Err(self.blocked_until_ns - now_ns);
        }
        let arrival = self.arrival_ns.max(now_ns);
        let ahead = arrival - now_ns;
        if ahead > self.burst_tolerance_ns {
            return Err(ahead - self.burst_tolerance_ns);
        }
        self.arrival_ns = arrival + self.interval_ns;
        Ok(())
    }

    /// The venue said to wait `wait_ns` from now. A shorter statement never shortens a longer
    /// one, and a wait that is not positive says nothing.
    pub fn retry_after(&mut self, now_ns: i128, wait_ns: i128) {
        if wait_ns > 0 {
            self.blocked_until_ns = self.blocked_until_ns.max(now_ns.saturating_add(wait_ns));
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resilience/budget_tests.rs"]
mod tests;
