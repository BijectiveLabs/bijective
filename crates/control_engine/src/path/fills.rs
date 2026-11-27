// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Recording fills on the control stream: once per execution identity, only for orders this
//! platform placed, and only when the order lifecycle accepts them.

use bijective_lifecycle::{Applied, Fill};

use super::{OrderPath, PathError};
use crate::wire::ControlEvent;

/// What came of offering one fill to the control stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillRecorded {
    /// Written, and folded into the order and its position.
    Recorded { seq: u64 },
    /// Already on the stream with the same content. Nothing was written.
    Duplicate,
    /// No intent of this platform names the order: an external execution, for reconciliation to
    /// report, never recorded as the platform's own.
    NotOurs,
    /// The lifecycle refused it (overfill, contradiction, a reused identity). Nothing was written.
    Refused { detail: String },
}

impl OrderPath {
    #[cfg(test)]
    pub(crate) fn ledger_for_tests(&self, venue: &str) -> bijective_lifecycle::Ledger {
        self.replayed.state().map(|r| r.orders().ledger_for(venue)).unwrap_or_default()
    }

    /// Checked against the lifecycle before anything is written, so the stream never holds a fill
    /// its own replay would refuse.
    pub fn record_fill(
        &mut self,
        client_id: &str,
        fill: &Fill,
        at_ns: i128,
    ) -> Result<FillRecorded, PathError> {
        let verdict = match self.replayed.state().and_then(|r| r.orders().order(client_id)) {
            None => return Ok(FillRecorded::NotOurs),
            Some(order) => {
                order.apply(&bijective_lifecycle::OrderEvent::Filled(fill.clone()), at_ns)
            }
        };
        match verdict {
            Ok(Applied::Duplicate) => Ok(FillRecorded::Duplicate),
            Err(e) => Ok(FillRecorded::Refused { detail: e.to_string() }),
            Ok(Applied::Transitioned { .. }) => {
                let seq = self.record(&ControlEvent::Filled {
                    client_id: client_id.to_string(),
                    fill: fill.clone(),
                    at_ns,
                })?;
                // A fill can finish an order or close a position. Either returns budget.
                self.settle_reservations()?;
                Ok(FillRecorded::Recorded { seq })
            }
        }
    }
}
