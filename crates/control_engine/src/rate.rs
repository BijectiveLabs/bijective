// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A venue's order rate: at most `orders` submits in any window of `per_ns`, judged on the
//! instants the order path's clock gives, so the same sequence is admitted the same way.
//!
//! The window is a log of the last submits, not a refilling allowance, so the bound holds over
//! every window and not only on average. A venue's own rate-limit answer closes the window until
//! the interval it stated, or one full window when it stated none.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderRate {
    pub orders: u32,
    pub per_ns: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateWindow {
    rate: OrderRate,
    sent: VecDeque<i128>,
    closed_until_ns: i128,
}

impl RateWindow {
    /// Precondition: `rate.orders` and `rate.per_ns` are positive. The configuration refuses others.
    #[must_use]
    pub fn new(rate: OrderRate) -> Self {
        Self { rate, sent: VecDeque::new(), closed_until_ns: i128::MIN }
    }

    /// `None` when a submit at `at_ns` fits the rate. Otherwise how long until one would.
    #[must_use]
    pub fn wait_ns(&self, at_ns: i128) -> Option<i128> {
        if at_ns < self.closed_until_ns {
            return Some(self.closed_until_ns - at_ns);
        }
        let in_window = self.sent.iter().filter(|t| at_ns - **t < self.rate.per_ns).count();
        if in_window < self.rate.orders as usize {
            return None;
        }
        // The oldest submit still in the window decides when a place opens.
        let oldest = self.sent.iter().find(|t| at_ns - **t < self.rate.per_ns)?;
        Some(oldest + self.rate.per_ns - at_ns)
    }

    /// Records a submit sent at `at_ns`, keeping only what a later window can still see.
    pub fn sent(&mut self, at_ns: i128) {
        self.sent.push_back(at_ns);
        while self.sent.len() > self.rate.orders as usize {
            self.sent.pop_front();
        }
    }

    /// The venue refused for rate: nothing is sent before `retry_after_ns`, or a full window.
    pub fn refused(&mut self, at_ns: i128, retry_after_ns: Option<i128>) {
        let wait = retry_after_ns.filter(|w| *w > 0).unwrap_or(self.rate.per_ns);
        self.closed_until_ns = self.closed_until_ns.max(at_ns.saturating_add(wait));
    }
}

#[cfg(test)]
#[path = "../tests/unit/rate_tests.rs"]
mod tests;
