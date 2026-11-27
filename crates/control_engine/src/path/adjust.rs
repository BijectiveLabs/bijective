// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Carrying out a position adjustment: checked against the venue's ledger first, then recorded,
//! so the stream never holds an adjustment its own replay would refuse.

use bijective_model::VenueSymbol;
use bijective_serialization::signal::{Command, PositionAdjustment};

use super::{OrderPath, PathError};
use crate::orders::cause_of;
use crate::wire::{Adjustment, ControlEvent};

impl OrderPath {
    /// Returns the refusal, or an empty string once the adjustment is recorded. The command's own
    /// identifier is the adjustment's identity, so issuing one command twice moves nothing twice.
    pub(super) fn adjust_position(
        &mut self,
        command: &Command,
        venue: &str,
        asked: &PositionAdjustment,
        at_ns: i128,
    ) -> Result<String, PathError> {
        if let Some(refusal) = refusal(asked) {
            return Ok(refusal);
        }
        let adjustment = Adjustment {
            venue: venue.to_string(),
            adjustment_id: command.id.clone(),
            actor: command.actor.clone(),
            instrument: VenueSymbol::new(asked.instrument.as_str()),
            side: asked.side,
            quantity_scaled: asked.quantity_scaled,
            price_scaled: asked.price_scaled,
            reason: asked.reason.clone(),
            owner: asked.owner.clone(),
        };
        let mut ledger =
            self.replayed.state().map(|r| r.orders().ledger_for(venue)).unwrap_or_default();
        if let Err(e) = ledger.adjust(
            &adjustment.instrument,
            cause_of(&adjustment),
            adjustment.side,
            adjustment.price_scaled,
            adjustment.quantity_scaled,
        ) {
            return Ok(format!("adjustment refused: {e}"));
        }
        self.record(&ControlEvent::PositionAdjusted { adjustment, at_ns })?;
        // An adjustment can close a position, which returns what its orders kept reserved.
        self.settle_reservations()?;
        Ok(String::new())
    }
}

/// An adjustment an auditor could not read, or that moves nothing, is refused before the ledger.
fn refusal(asked: &PositionAdjustment) -> Option<String> {
    if asked.instrument.is_empty() {
        return Some("adjustment refused: it names no instrument".into());
    }
    if asked.reason.trim().is_empty() {
        return Some("adjustment refused: it states no reason".into());
    }
    if asked.quantity_scaled <= 0 || asked.price_scaled <= 0 {
        return Some(format!(
            "adjustment refused: quantity {} and price {} must both be positive",
            asked.quantity_scaled, asked.price_scaled
        ));
    }
    None
}
