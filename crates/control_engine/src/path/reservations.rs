// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Returning budget a finished order no longer needs: what it reserved beyond its filled share,
//! and the rest once the position it opened is closed.
//!
//! Derived, not tracked: the sweep compares each finished order with what its decision still
//! holds, so running it twice releases nothing twice, and a crash between an order finishing and
//! its release is repaired by the sweep at the next start.

use bijective_limits::Amount;
use bijective_risk::limit_state::{Holding, ReleaseReason};

use super::{OrderPath, PathError};
use crate::orders::Finished;

impl OrderPath {
    /// Releases, for every finished order, what its decision holds beyond the filled share of what
    /// it took. Returns how many releases were written. Nothing is written before the limit state
    /// is rebuilt.
    pub fn settle_reservations(&mut self) -> Result<usize, PathError> {
        if !self.limits.may_trade() {
            return Ok(0);
        }
        let finished = self.replayed.state().map(|r| r.orders().finished()).unwrap_or_default();
        let mut written = 0;
        for order in &finished {
            for (scope, kind, holding) in self.limits.held_by(&order.decision_id) {
                let Some(excess) = excess(order, holding) else { continue };
                let reason = if order.position_closed {
                    ReleaseReason::PositionClosed
                } else {
                    ReleaseReason::OrderCancelled
                };
                // Never more than the scope holds in total: the limit state would refuse it, and
                // the refusal would stop the sweep and with it the start.
                let consumed = self.limits.consumed(&scope, kind).scaled();
                let excess = Amount::from_scaled(excess.scaled().min(consumed));
                if excess.scaled() <= 0 {
                    continue;
                }
                self.limits
                    .release(
                        &mut self.limit_journal,
                        scope,
                        kind,
                        excess,
                        reason,
                        Some(&order.decision_id),
                    )
                    .map_err(PathError::Consume)?;
                written += 1;
            }
        }
        Ok(written)
    }
}

/// What is held beyond what the order still needs: nothing once its position is closed, else the
/// filled share, rounded so the share kept is never less than exact:
/// rounding the other way would return budget a fill still uses. `None` when nothing is in excess,
/// or when the share cannot be computed exactly, which keeps the budget reserved.
fn excess(order: &Finished, holding: Holding) -> Option<Amount> {
    if order.ordered_scaled <= 0 || order.filled_scaled < 0 {
        return None;
    }
    if order.position_closed {
        return (holding.held.scaled() > 0).then_some(holding.held);
    }
    let filled = order.filled_scaled.min(order.ordered_scaled);
    let share =
        holding.taken.scaled().checked_mul(filled)?.checked_add(order.ordered_scaled - 1)?
            / order.ordered_scaled;
    let excess = holding.held.scaled() - share;
    (excess > 0).then(|| Amount::from_scaled(excess))
}
