// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the platform's own record says about the order things happened in.
//! Research integrity, fill half. Every clock compared is the platform's own, never a proposal's instant.

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;

use bijective_control_engine::wire::{ControlEvent, decode, names_a_proposal};
use bijective_event_store::store::{StoreError, read_range};

/// What the record says happened out of order, or not at all. Findings, not refusals: the record is already written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A venue answer recorded before the intent that asked for it. The intent is written before the venue call.
    SettledBeforeItsIntent { client_id: String, intent_at_ns: i128, settled_at_ns: i128 },
    /// A venue's answer about an order nothing here asked for.
    SettledWithNoIntent { client_id: String, at_ns: i128 },
    /// A fill recorded for an order nothing here placed: the control stream records only its own.
    FilledWithNoIntent { client_id: String, at_ns: i128 },
    /// Evidence settled an order nothing here placed: reconciliation records only its own orders.
    ReconciledWithNoIntent { client_id: String, at_ns: i128 },
    /// One proposal acted on twice: what a restart must not cause.
    ProposalActedOnTwice { from_position: u64, first_at_ns: i128, again_at_ns: i128 },
    /// An act on a decision the proposal stream does not hold. Intents and refusals both advance the mark.
    ActNamesNoProposal { from_position: u64, proposals_held: u64 },
    /// A benchmark for a decision the proposal stream does not hold: a figure measured against nothing.
    BenchmarkNamesNoProposal { from_position: u64, proposals_held: u64 },
}

impl core::fmt::Display for Finding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Finding::SettledBeforeItsIntent { client_id, intent_at_ns, settled_at_ns } => write!(
                f,
                "{client_id} settled at {settled_at_ns}, {} ns before the intent that asked for it",
                intent_at_ns.saturating_sub(*settled_at_ns)
            ),
            Finding::SettledWithNoIntent { client_id, at_ns } => {
                write!(f, "{client_id} settled at {at_ns} and nothing here asked for it")
            }
            Finding::FilledWithNoIntent { client_id, at_ns } => {
                write!(f, "{client_id} has a fill recorded at {at_ns} and no intent before it")
            }
            Finding::ReconciledWithNoIntent { client_id, at_ns } => {
                write!(
                    f,
                    "{client_id} was settled by evidence at {at_ns} and has no intent before it"
                )
            }
            Finding::ProposalActedOnTwice { from_position, first_at_ns, again_at_ns } => write!(
                f,
                "proposal {from_position} was acted on at {first_at_ns} and again at {again_at_ns}"
            ),
            Finding::ActNamesNoProposal { from_position, proposals_held } => write!(
                f,
                "an act names proposal {from_position} and the stream holds {proposals_held}"
            ),
            Finding::BenchmarkNamesNoProposal { from_position, proposals_held } => write!(
                f,
                "a benchmark names proposal {from_position} and the stream holds {proposals_held}"
            ),
        }
    }
}

#[derive(Debug)]
pub enum AuditError {
    Store(StoreError),
    /// An entry this build cannot read. Refused, never skipped: an audit must not report clean on a record it did not finish.
    Undecodable {
        at: u64,
    },
}

impl core::fmt::Display for AuditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AuditError::Store(e) => write!(f, "the record could not be read: {e:?}"),
            AuditError::Undecodable { at } => {
                write!(f, "the control entry at {at} is not one this build can read")
            }
        }
    }
}

impl std::error::Error for AuditError {}

/// Read the record and report what happened out of order.
/// An empty list covers only these shapes. The read count tells an empty stream from a full one.
pub fn audit(
    root: &Path,
    control: &str,
    proposals: &str,
) -> Result<(usize, Vec<Finding>), AuditError> {
    let entries = read_range(root, control, 0, u64::MAX).map_err(AuditError::Store)?;
    let held = read_range(root, proposals, 0, u64::MAX).map_err(AuditError::Store)?.len() as u64;

    let mut findings = Vec::new();
    let mut intents: BTreeMap<String, i128> = BTreeMap::new();
    let mut acts = Acts { held, ..Acts::default() };
    let mut read = 0;

    for entry in &entries {
        let event =
            decode(&entry.payload).map_err(|_| AuditError::Undecodable { at: entry.seq })?;
        read += 1;
        match event {
            ControlEvent::Held { proposal, from_position, at_ns } => {
                acts.act(&mut findings, &proposal.client_id, from_position, at_ns);
                acts.linked.insert(proposal.client_id);
            }
            ControlEvent::Intent { decision_id, request, from_position, at_ns, .. } => {
                acts.act(&mut findings, &decision_id, from_position, at_ns);
                intents.insert(request.client_id.clone(), at_ns);
            }
            // A refusal advances the mark like an intent, so a second act is the same finding.
            ControlEvent::Refused { decision_id, from_position, at_ns, .. } => {
                acts.act(&mut findings, &decision_id, from_position, at_ns);
            }
            ControlEvent::Settled { client_id, at_ns, .. } => match intents.get(&client_id) {
                None => findings.push(Finding::SettledWithNoIntent { client_id, at_ns }),
                Some(asked) if at_ns < *asked => {
                    findings.push(Finding::SettledBeforeItsIntent {
                        client_id,
                        intent_at_ns: *asked,
                        settled_at_ns: at_ns,
                    });
                }
                Some(_) => {}
            },
            ControlEvent::Captured { from_position, .. } => {
                if from_position >= held {
                    findings.push(Finding::BenchmarkNamesNoProposal {
                        from_position,
                        proposals_held: held,
                    });
                }
            }
            ControlEvent::Filled { client_id, at_ns, .. } => {
                if !intents.contains_key(&client_id) {
                    findings.push(Finding::FilledWithNoIntent { client_id, at_ns });
                }
            }
            ControlEvent::Reconciled { client_id, at_ns, .. } => {
                if !intents.contains_key(&client_id) {
                    findings.push(Finding::ReconciledWithNoIntent { client_id, at_ns });
                }
            }
            ControlEvent::Withdrawn { .. }
            | ControlEvent::ReconciliationCompleted { .. }
            | ControlEvent::PositionAdjusted { .. }
            | ControlEvent::DayOpened { .. }
            | ControlEvent::PortfolioValued { .. }
            | ControlEvent::ScopesValued { .. }
            | ControlEvent::DayPeak { .. }
            | ControlEvent::AccountCharged { .. }
            | ControlEvent::InstrumentScales { .. }
            | ControlEvent::VenueParticipation { .. }
            | ControlEvent::SafeStateDriven { .. }
            | ControlEvent::SafeStatePostcondition { .. }
            | ControlEvent::CommandApplied { .. } => {}
        }
    }

    Ok((read, findings))
}

/// The acts on proposals seen so far. A held proposal's releases (decision 0013) act on the
/// position its hold already acted on. A flatten order names no proposal.
#[derive(Default)]
struct Acts {
    held: u64,
    acted: BTreeMap<u64, i128>,
    linked: std::collections::BTreeSet<String>,
}

impl Acts {
    fn act(
        &mut self,
        findings: &mut Vec<Finding>,
        decision_id: &str,
        from_position: u64,
        at_ns: i128,
    ) {
        let release = self.linked.contains(decision_id) && self.acted.contains_key(&from_position);
        if names_a_proposal(decision_id) && !release {
            mark_act(&mut self.acted, findings, from_position, self.held, at_ns);
        }
    }
}

/// An act (an intent or a refusal) on a proposal: it must name a held proposal, and only once.
fn mark_act(
    acted: &mut BTreeMap<u64, i128>,
    findings: &mut Vec<Finding>,
    from_position: u64,
    held: u64,
    at_ns: i128,
) {
    if from_position >= held {
        findings.push(Finding::ActNamesNoProposal { from_position, proposals_held: held });
    }
    match acted.get(&from_position) {
        Some(first) => findings.push(Finding::ProposalActedOnTwice {
            from_position,
            first_at_ns: *first,
            again_at_ns: at_ns,
        }),
        None => {
            acted.insert(from_position, at_ns);
        }
    }
}
