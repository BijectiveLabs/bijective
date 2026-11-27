// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Safe states driven whose exposure postcondition is still unanswered.
//! Rebuilt from the control stream at start so a restart cannot forget it had halted.

use std::collections::BTreeMap;

use bijective_event_store::projection::Projection;
use bijective_limits::{LimitKind, SafeStateReason, Scope, Threshold};

use crate::wire::{ControlEvent, decode};

#[derive(Debug)]
pub enum UnverifiedError {
    Decode(bijective_serialization::codec::DecodeError),
}

impl std::fmt::Display for UnverifiedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnverifiedError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

impl std::error::Error for UnverifiedError {}

/// The figures a safe state was driven on, awaiting a later observation.
/// Keyed on the figure, not the record: `Projection::apply` has no position.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unverified {
    awaiting: BTreeMap<(Scope, LimitKind), Threshold>,
}

impl Unverified {
    pub fn awaiting(&self) -> impl Iterator<Item = (&Scope, LimitKind, Threshold)> {
        self.awaiting.iter().map(|((scope, kind), before)| (scope, *kind, *before))
    }
}

impl Projection for Unverified {
    type Event = ControlEvent;
    type Error = UnverifiedError;

    fn decode(payload: &[u8]) -> Result<ControlEvent, UnverifiedError> {
        decode(payload).map_err(UnverifiedError::Decode)
    }

    fn apply(&mut self, event: &ControlEvent) -> Result<(), UnverifiedError> {
        match event {
            // Only a breach carries a measured figure. Other drives have nothing to compare against.
            ControlEvent::SafeStateDriven {
                reason: SafeStateReason::Breached(breach),
                scope: Some(scope),
                ..
            } => {
                // A later drive replaces the earlier one: its before-figure is the more recent observation.
                self.awaiting.insert((scope.clone(), breach.kind), breach.observed);
            }
            ControlEvent::SafeStatePostcondition { scope, kind, .. } => {
                self.awaiting.remove(&(scope.clone(), *kind));
            }
            _ => {}
        }
        Ok(())
    }
}
