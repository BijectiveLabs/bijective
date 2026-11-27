// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reconciliation against one venue, recorded on the control stream: every state change the
//! evidence settled, every fill it proved, and the verdict with its findings.
//!
//! Written as events so the projection rebuilt at the next start holds what this pass decided. A
//! pass that was never recorded would be repeated on stale belief, or forgotten.

use bijective_lifecycle::{Finding, OrderEvent, Reconciliation, VenueEvidence, Verdict, reconcile};

use bijective_limits::{SafeState, SafeStateReason};

use super::{OrderPath, PathError};
use crate::wire::{ControlEvent, EvidenceRead, Settlement};

/// An intent is stamped on this node's clock, and the venue stamps its orders and fills on its
/// own. A window that starts at the intent misses an order the venue stamped earlier (seen
/// 2026-11-17: an order created 46 ms before its intent's instant was never read, and the node
/// halted on a position no fill explained). Reading a minute earlier costs a little history. A
/// missed fill costs a halt.
pub const EVIDENCE_CLOCK_MARGIN_NS: i128 = 60_000_000_000;

/// What one recorded pass concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    /// Order state changes and fills written to the control stream.
    pub recorded: usize,
}

impl OrderPath {
    /// Reconciles the orders routed to `venue` against what it reported, records the outcome, and
    /// returns it. Postcondition: the replayed orders equal the reconciled ledger.
    pub fn reconcile_venue(
        &mut self,
        venue: &str,
        evidence: &VenueEvidence,
        at_ns: i128,
    ) -> Result<Reconciled, PathError> {
        let ledger =
            self.replayed.state().map(|r| r.orders().ledger_for(venue)).unwrap_or_default();
        let pass = reconcile(&ledger, evidence, at_ns);
        if pass.findings.iter().any(|f| matches!(f, Finding::FillsPending { .. })) {
            self.awaiting_fills.insert(venue.to_string());
        } else {
            self.awaiting_fills.remove(venue);
        }
        let recorded = self.record_settled(venue, &pass, at_ns)?;
        self.record_charges(venue, evidence, at_ns)?;
        self.record_verdict(venue, pass.verdict, &pass.findings, EvidenceRead::Read, at_ns)?;
        if pass.verdict == Verdict::Diverged {
            self.halt_for(SafeStateReason::ReconciliationDiverged, at_ns)?;
        }
        // An order the evidence finished returns what it reserved beyond its fills.
        self.settle_reservations()?;
        Ok(Reconciled { verdict: pass.verdict, findings: pass.findings, recorded })
    }

    /// A pass that read no evidence is recorded as unverified with the reason and how the read
    /// went, so the record shows a pass was attempted and why it settled nothing. `Failed` holds
    /// resume until a later read succeeds.
    pub fn reconciliation_unavailable(
        &mut self,
        venue: &str,
        reason: &str,
        evidence: EvidenceRead,
        at_ns: i128,
    ) -> Result<(), PathError> {
        let findings = vec![Finding::Refused { detail: reason.to_string() }];
        if evidence == EvidenceRead::Failed {
            self.halt_for(SafeStateReason::EvidenceUnread, at_ns)?;
        }
        self.record_verdict(venue, Verdict::Unverified, &findings, evidence, at_ns)
    }

    fn record_settled(
        &mut self,
        venue: &str,
        pass: &Reconciliation,
        at_ns: i128,
    ) -> Result<usize, PathError> {
        for (client_id, event) in &pass.events {
            let recorded = match (event, Settlement::of(event)) {
                (OrderEvent::Filled(fill), _) => {
                    ControlEvent::Filled { client_id: client_id.clone(), fill: fill.clone(), at_ns }
                }
                (_, Some(settlement)) => ControlEvent::Reconciled {
                    venue: venue.to_string(),
                    client_id: client_id.clone(),
                    settlement,
                    at_ns,
                },
                // Reconciliation applies no other event. One it did would be a change in the
                // lifecycle crate, refused here rather than written in a form replay misreads.
                (other, None) => {
                    return Err(PathError::Replay(format!(
                        "reconciliation applied {} to {client_id}, which has no record",
                        other.name()
                    )));
                }
            };
            self.record(&recorded)?;
        }
        Ok(pass.events.len())
    }

    /// Each account charge not recorded before. A charge read again in a later window is not.
    fn record_charges(
        &mut self,
        venue: &str,
        evidence: &VenueEvidence,
        at_ns: i128,
    ) -> Result<(), PathError> {
        for charge in &evidence.charges {
            let known = self
                .replayed
                .state()
                .is_some_and(|r| r.orders().has_charge(venue, &charge.charge_id));
            if !known {
                self.record(&ControlEvent::AccountCharged {
                    venue: venue.to_string(),
                    charge: charge.clone(),
                    at_ns,
                })?;
            }
        }
        Ok(())
    }

    fn record_verdict(
        &mut self,
        venue: &str,
        verdict: Verdict,
        findings: &[Finding],
        evidence: EvidenceRead,
        at_ns: i128,
    ) -> Result<(), PathError> {
        self.record(&ControlEvent::ReconciliationCompleted {
            venue: venue.to_string(),
            verdict,
            findings: findings.iter().map(ToString::to_string).collect(),
            evidence,
            at_ns,
        })?;
        Ok(())
    }

    /// Halts a trading platform because what a venue holds is not known to agree with the record.
    /// Already halted, nothing is written: a standing divergence re-checked each minute is one
    /// record, not one per pass.
    fn halt_for(&mut self, reason: SafeStateReason, at_ns: i128) -> Result<(), PathError> {
        if self.trading_state() == SafeState::Halted {
            return Ok(());
        }
        self.escalate(SafeState::Halted, reason, None, at_ns)
    }

    /// Venues where a pass could still settle something. See `Orders::unsettled`.
    #[must_use]
    pub fn unsettled_venues(&self) -> Vec<String> {
        self.replayed.state().map(|r| r.orders().unsettled()).unwrap_or_default()
    }

    /// The instant evidence must reach back to for `venue`: its latest aligned pass, or its first
    /// routed intent where none is later, less `EVIDENCE_CLOCK_MARGIN_NS`. Bounded by the last
    /// agreement rather than the first intent, so a node that runs for months reads what changed
    /// since, not its whole history. `None` when nothing was routed there, so positions alone are
    /// compared.
    #[must_use]
    pub fn evidence_since_ns(&self, venue: &str) -> Option<i128> {
        self.replayed
            .state()
            .and_then(|r| r.orders().evidence_watermark_ns(venue))
            .map(|from| from - EVIDENCE_CLOCK_MARGIN_NS)
    }

    /// Venues whose latest pass saw an order filled ahead of its fills, owed a read on the next
    /// pass: a fill a strategy waits on is otherwise heard an interval late.
    #[must_use]
    pub fn awaiting_fills(&self) -> Vec<String> {
        self.awaiting_fills.iter().cloned().collect()
    }

    /// The orders the record routed to `venue`, for its adapter to take back at start.
    #[must_use]
    pub fn recorded_for(&self, venue: &str) -> Vec<bijective_venue::contract::Recorded> {
        self.replayed.state().map(|r| r.orders().recorded_for(venue)).unwrap_or_default()
    }
}
