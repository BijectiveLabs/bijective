// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order rate on the order path: a submit the venue's window has no room for is refused
//! before any budget is drawn, and every submit sent is counted against it.

use bijective_venue::normalise::{Rejection, VenueOutcome};

use super::OrderPath;
use crate::rate::{OrderRate, RateWindow};
use crate::wire::RefusalReason;

impl OrderPath {
    /// Bounds the submits to `venue`. A restart starts the window empty.
    pub fn set_order_rate(&mut self, venue: &str, rate: OrderRate) {
        self.rates.insert(venue.to_string(), RateWindow::new(rate));
    }

    pub(super) fn rate_refusal(&self, venue: &str, at_ns: i128) -> Option<RefusalReason> {
        let wait = self.rates.get(venue)?.wait_ns(at_ns)?;
        Some(RefusalReason::RateLimited { retry_after_ns: wait })
    }

    /// Counted whatever the answer: a refused or unknown submit still used the venue's rate.
    pub(super) fn count_submit(&mut self, venue: &str, outcome: &VenueOutcome, at_ns: i128) {
        let Some(window) = self.rates.get_mut(venue) else { return };
        window.sent(at_ns);
        if let VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns }) = outcome {
            window.refused(at_ns, *retry_after_ns);
        }
    }
}
