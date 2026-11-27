// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One order a flatten command asked for, on the order path (decision 0010).

use bijective_common::clock::Clock;
use bijective_model::{Clearance, OrderRequest};
use bijective_venue::contract::VenueExecution;

use super::{ControlEvent, OrderPath, PathError, Placement, RefusalReason, instrument_refusal};

/// What a flatten order carries beside its request.
#[derive(Debug, Clone, Copy)]
pub struct FlattenOrder<'a> {
    /// The command's decision identifier (`wire::flatten_decision_id`), shared by its orders.
    pub decision_id: &'a str,
    pub venue: &'a str,
    pub request: &'a OrderRequest,
}

impl OrderPath {
    /// Whether a flatten order for this position was refused already in this process.
    #[must_use]
    pub fn flatten_refused(&self, venue: &str, instrument: &bijective_model::VenueSymbol) -> bool {
        self.flatten_refused.contains(&(venue.to_string(), instrument.clone()))
    }

    /// Submits one reduce-only market order a carried-out flatten command asked for.
    ///
    /// Judged on the record, not on the risk figures: a breach, an unjudged limit or stale figures
    /// are why an operator closes positions, so they do not stop it, and neither does the trading
    /// state. The reduce-only check against the record, the venue's instrument rules and its order
    /// rate do. Nothing is drawn from a budget. Written as an intent before the call and settled
    /// after it, like every order. The records name no proposal.
    pub async fn flatten_one(
        &mut self,
        venue: &mut dyn VenueExecution,
        clock: &dyn Clock,
        order: &FlattenOrder<'_>,
    ) -> Result<Placement, PathError> {
        let at_ns = clock.now_ns();
        let request = order.request;
        let refusal = if !request.reduce_only || request.price_scaled.is_some() {
            Some(RefusalReason::NotAdmitted {
                detail: "a flatten order is a reduce-only market order".into(),
            })
        } else {
            instrument_refusal(venue.declaration(), request)
                .or_else(|| {
                    let declaration = venue.declaration();
                    self.does_not_reduce(declaration, order.venue, request, order.decision_id)
                })
                .or_else(|| self.rate_refusal(order.venue, at_ns))
        };
        if let Some(reason) = refusal {
            // Recorded once per position in this process: the pass tries again each heartbeat.
            self.flatten_refused.insert((order.venue.to_string(), request.instrument.clone()));
            return self.refuse(order.decision_id, 0, reason, at_ns);
        }

        self.declare_scales(order.venue, venue.declaration(), &request.instrument, at_ns)?;
        self.record(&ControlEvent::Intent {
            decision_id: order.decision_id.to_string(),
            venue: Some(order.venue.to_string()),
            owner: None,
            request: request.clone(),
            from_position: 0,
            at_ns,
        })?;
        let clearance = Clearance::for_operator_reduction(at_ns);
        let outcome = venue.submit(&clearance, request, at_ns).await;
        self.count_submit(order.venue, &outcome, at_ns);
        self.record(&ControlEvent::Settled {
            client_id: request.client_id.clone(),
            outcome: outcome.clone(),
            at_ns: clock.now_ns(),
        })?;
        Ok(Placement::Sent { outcome })
    }
}
