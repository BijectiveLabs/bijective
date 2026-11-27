// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Recording the portfolio's figures on the control stream, for the readers that show them.

use super::{ControlEvent, OrderPath, PathError};
use crate::wire::{ScopeFigure, Valued};

impl OrderPath {
    /// Records `figures` when `summary::due` says they are worth recording again. The last
    /// summary is held in memory, so a restart records its first pass's figures once more.
    pub fn record_valuation(&mut self, figures: Valued, at_ns: i128) -> Result<(), PathError> {
        let last = self.last_valued.as_ref().map(|(v, t)| (v, *t));
        if !crate::summary::due(last, &figures, at_ns) {
            return Ok(());
        }
        self.record(&ControlEvent::PortfolioValued { figures, at_ns })?;
        self.last_valued = Some((figures, at_ns));
        Ok(())
    }

    /// Records the figures below the portfolio when `summary::scoped_due` says so. The same
    /// rule as the portfolio summary, so the console shows a breach as it is judged.
    pub fn record_scopes(
        &mut self,
        figures: Vec<ScopeFigure>,
        at_ns: i128,
    ) -> Result<(), PathError> {
        let last = self.last_scopes.as_ref().map(|(v, t)| (v.as_slice(), *t));
        if !crate::summary::scoped_due(last, &figures, at_ns) {
            return Ok(());
        }
        self.record(&ControlEvent::ScopesValued { figures: figures.clone(), at_ns })?;
        self.last_scopes = Some((figures, at_ns));
        Ok(())
    }
}
