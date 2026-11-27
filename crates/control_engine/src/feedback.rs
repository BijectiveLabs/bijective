// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The control stream's events about proposals, mirrored for the strategies that made them.
//!
//! Precondition: every event this engine records passes through `Mirror::observe` with its control
//! position, in order. Postcondition: the feedback stream holds one record for each event about a
//! proposal's order, in control order. A crash between the two appends leaves the mirror behind,
//! never ahead, and `Mirror::open` completes it from the control stream before anything new is
//! recorded. The control stream remains the record: a strategy that reads this stream reads a
//! projection of it.

use std::collections::BTreeMap;
use std::path::Path;

use bijective_event_store::store::{StreamWriter, read_range};
use bijective_serialization::signal::feedback::{
    Feedback, Outcome, decode as decode_feedback, encode as encode_feedback, stream_for,
};
use bijective_venue::normalise::VenueOutcome;

use crate::path::PathError;
use crate::wire::{ControlEvent, Settlement, decode, names_a_proposal};

/// The proposal position, the strategy's identifier, outcome, detail and time one control event
/// says, if it is about a proposal's order. `positions` maps each order's client identifier to its
/// proposal and the identifier the strategy proposed under, and is grown by intents, so later
/// events about the order name the proposal as the strategy named it.
pub fn mirrored(
    event: &ControlEvent,
    positions: &mut BTreeMap<String, (u64, String)>,
) -> Option<(u64, Option<String>, Outcome, String, i128)> {
    let about = |client_id: &str, positions: &BTreeMap<String, (u64, String)>| {
        positions.get(client_id).map(|(p, named)| (*p, Some(named.clone())))
    };
    if let ControlEvent::Intent { decision_id, .. } | ControlEvent::Refused { decision_id, .. } =
        event
        && !names_a_proposal(decision_id)
    {
        return None;
    }
    let (position, client_id, outcome, detail, at_ns) = match event {
        ControlEvent::Intent { decision_id, request, from_position, at_ns, .. } => {
            positions.insert(request.client_id.clone(), (*from_position, decision_id.clone()));
            let id = Some(decision_id.clone());
            (*from_position, id, Outcome::Submitted, String::new(), *at_ns)
        }
        ControlEvent::Refused { reason, from_position, at_ns, .. } => {
            (*from_position, None, Outcome::Refused, format!("{reason:?}"), *at_ns)
        }
        ControlEvent::Settled { client_id, outcome, at_ns } => {
            let (position, id) = about(client_id, positions)?;
            let (outcome, detail) = answered(outcome);
            (position, id, outcome, detail, *at_ns)
        }
        ControlEvent::Filled { client_id, fill, at_ns } => {
            let (position, id) = about(client_id, positions)?;
            let filled = Outcome::Filled {
                fill_id: fill.fill_id.clone(),
                quantity_scaled: fill.quantity_scaled,
                price_scaled: fill.price_scaled,
            };
            (position, id, filled, String::new(), *at_ns)
        }
        ControlEvent::Withdrawn { client_id, outcome, at_ns, .. } => {
            let (position, id) = about(client_id, positions)?;
            (position, id, Outcome::CancelRequested, format!("{outcome:?}"), *at_ns)
        }
        ControlEvent::Reconciled { client_id, settlement, at_ns, .. } => {
            let (position, id) = about(client_id, positions)?;
            let (outcome, detail) = settled(settlement);
            (position, id, outcome, detail, *at_ns)
        }
        _ => return None,
    };
    Some((position, client_id, outcome, detail, at_ns))
}

fn answered(outcome: &VenueOutcome) -> (Outcome, String) {
    match outcome {
        VenueOutcome::Accepted => (Outcome::Accepted, String::new()),
        VenueOutcome::Rejected(why) => (Outcome::Rejected, format!("{why:?}")),
        VenueOutcome::NotSent => (Outcome::Rejected, "not sent".to_string()),
        VenueOutcome::Unknown(why) => (Outcome::Unknown, format!("{why:?}")),
    }
}

fn settled(settlement: &Settlement) -> (Outcome, String) {
    match settlement {
        Settlement::ReconciliationStarted => (Outcome::Reconciling, String::new()),
        Settlement::Accepted { .. } => (Outcome::Accepted, String::new()),
        Settlement::Rejected { reason } => (Outcome::Rejected, reason.clone()),
        Settlement::Canceled => (Outcome::Canceled, String::new()),
        Settlement::Expired => (Outcome::Expired, String::new()),
    }
}

/// The feedback stream's writer and what it has mirrored so far.
pub struct Mirror {
    writer: StreamWriter,
    positions: BTreeMap<String, (u64, String)>,
    /// The last control position mirrored. `None` before the first.
    through: Option<u64>,
}

impl Mirror {
    /// Opens the mirror of control stream `control` and completes it: every control event after
    /// the last one mirrored is mirrored now. An unreadable record in either stream refuses the
    /// open, so a strategy is never told less than happened without anyone knowing.
    pub fn open(root: &Path, control: &str) -> Result<Self, PathError> {
        let name = stream_for(control);
        let through = read_range(root, &name, 0, u64::MAX)
            .map_err(PathError::Store)?
            .last()
            .map(|entry| decode_feedback(&entry.payload).map(|f| f.control_position))
            .transpose()
            .map_err(|e| PathError::Replay(format!("the feedback stream: {e:?}")))?;
        let mut mirror =
            Self { writer: StreamWriter::open(root, &name)?, positions: BTreeMap::new(), through };
        for entry in read_range(root, control, 0, u64::MAX).map_err(PathError::Store)? {
            let event = decode(&entry.payload)
                .map_err(|e| PathError::Replay(format!("control position {}: {e:?}", entry.seq)))?;
            mirror.observe(entry.seq, &event)?;
        }
        Ok(mirror)
    }

    /// Mirrors one recorded control event. One already mirrored only teaches the positions.
    pub fn observe(
        &mut self,
        control_position: u64,
        event: &ControlEvent,
    ) -> Result<(), PathError> {
        let said = mirrored(event, &mut self.positions);
        if self.through.is_some_and(|t| control_position <= t) {
            return Ok(());
        }
        if let Some((proposal_position, client_id, outcome, detail, at_ns)) = said {
            let record = Feedback {
                proposal_position,
                client_id,
                outcome,
                reason: detail,
                control_position,
                at_ns,
            };
            let payload = encode_feedback(&record).map_err(PathError::Encode)?;
            self.writer.append(&payload)?;
        }
        self.through = Some(control_position);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/feedback_tests.rs"]
mod tests;
