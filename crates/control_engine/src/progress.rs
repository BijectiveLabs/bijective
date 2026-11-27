// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! How far the engine has acted through the proposal stream.
//! Rebuilt at start so a restart resumes instead of placing every order twice.

use bijective_event_store::projection::Projection;

use crate::wire::{ControlEvent, decode, names_a_proposal};

#[derive(Debug)]
pub enum ProgressError {
    Decode(bijective_serialization::codec::DecodeError),
}

impl std::fmt::Display for ProgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

impl std::error::Error for ProgressError {}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Progress {
    through: Option<u64>,
    commands_through: Option<u64>,
    /// Identifiers of commands already carried out or refused, so a repeat is told from a first.
    applied: std::collections::BTreeSet<String>,
}

impl From<Option<u64>> for Progress {
    fn from(through: Option<u64>) -> Self {
        Self { through, commands_through: None, applied: std::collections::BTreeSet::new() }
    }
}

impl Progress {
    /// The first proposal position not acted on. `None` means nothing acted on, since zero is a real position.
    #[must_use]
    pub fn resume_from(&self) -> u64 {
        self.through.map_or(0, |t| t + 1)
    }

    #[must_use]
    pub fn through(&self) -> Option<u64> {
        self.through
    }

    #[must_use]
    pub fn has_applied(&self, command_id: &str) -> bool {
        self.applied.contains(command_id)
    }

    /// The first command position not carried out. Separate stream, separate mark.
    #[must_use]
    pub fn commands_from(&self) -> u64 {
        self.commands_through.map_or(0, |t| t + 1)
    }
}

impl Projection for Progress {
    type Event = ControlEvent;
    type Error = ProgressError;

    fn decode(payload: &[u8]) -> Result<ControlEvent, ProgressError> {
        decode(payload).map_err(ProgressError::Decode)
    }

    fn apply(&mut self, event: &ControlEvent) -> Result<(), ProgressError> {
        // Highest seen, not last applied: a mark that could move backwards would act on a proposal twice.
        if let ControlEvent::CommandApplied { from_position, command_id, .. } = event {
            self.applied.insert(command_id.clone());
            self.commands_through =
                Some(self.commands_through.map_or(*from_position, |held| held.max(*from_position)));
            return Ok(());
        }

        if let ControlEvent::Intent { decision_id, .. } | ControlEvent::Refused { decision_id, .. } =
            event
            && !names_a_proposal(decision_id)
        {
            return Ok(());
        }
        let seen = match event {
            ControlEvent::Intent { from_position, .. }
            | ControlEvent::Held { from_position, .. }
            | ControlEvent::Refused { from_position, .. } => *from_position,
            // Names no proposal already marked. Advancing would credit progress to the wrong record.
            ControlEvent::Settled { .. }
            // A withdrawal or a fill names no proposal.
            | ControlEvent::Withdrawn { .. }
            | ControlEvent::Filled { .. }
            | ControlEvent::AccountCharged { .. }
            | ControlEvent::InstrumentScales { .. }
            | ControlEvent::Reconciled { .. }
            | ControlEvent::ReconciliationCompleted { .. }
            | ControlEvent::PositionAdjusted { .. }
            | ControlEvent::DayOpened { .. }
            | ControlEvent::PortfolioValued { .. }
            | ControlEvent::ScopesValued { .. }
            | ControlEvent::DayPeak { .. }
            | ControlEvent::VenueParticipation { .. }
            | ControlEvent::SafeStateDriven { .. }
            | ControlEvent::SafeStatePostcondition { .. }
            | ControlEvent::CommandApplied { .. }
            // Belongs to a decision the beside intent already marks.
            | ControlEvent::Captured { .. } => return Ok(()),
        };

        self.through = Some(self.through.map_or(seen, |held| held.max(seen)));
        Ok(())
    }
}
