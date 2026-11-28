// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The price collar: a limit price that would cross the mark by more than the desk allows is
//! refused before the order leaves, so a mis-scaled price cannot sweep the book.

use bijective_model::{Proposal, Side};

use super::admission::{MandateBook, Refusal};
use crate::valuation::Mark;

/// The collar a desk applies when its configuration names none: 5 percent through the mark.
pub const DEFAULT_COLLAR_BPS: u32 = 500;

/// The widest collar a desk may declare: a buy at twice the mark, a sell at nothing.
pub const WIDEST_COLLAR_BPS: u32 = 10_000;

/// The mark a limit price is judged against, and how old it may be at the judging instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollarMark {
    /// The instrument's mark, at its own price scale. `None` when the node holds none.
    pub mark: Option<Mark>,
    pub as_of_ns: i128,
    pub max_age_ns: i128,
}

/// Whether the proposal's limit price lies within its desk's collar around the mark.
///
/// Every order needs a mark no older than `max_age_ns`, or it is refused: an order decided on a
/// market the node cannot see as current is not placed, whatever its kind. Only the side that
/// crosses is bounded: a buy above the mark or a sell below it. A passive price far from the mark
/// rests without harm. A market order carries no price the venue sees, so its price is not
/// collared. Its cap and the desk's per-order maximum bound it. A stop must not already be
/// crossed, and a stop-limit's price is collared around its stop, where it becomes a limit order.
pub fn within_collar(
    mandates: &MandateBook,
    proposal: &Proposal,
    at: &CollarMark,
) -> Result<(), Refusal> {
    let Some(mark) = at.mark.filter(|m| m.price_scaled > 0) else {
        return Err(Refusal::NoMarkForCollar { instrument: proposal.instrument.clone() });
    };
    let age_ns = at.as_of_ns - mark.observed_at_ns;
    if age_ns > at.max_age_ns {
        return Err(Refusal::MarkTooOldForCollar { age_ns, max_age_ns: at.max_age_ns });
    }

    let stop = proposal.terms.stop_price_scaled;
    if proposal.price_scaled.is_none() && stop.is_none() {
        return Ok(());
    }
    let mandate = mandates
        .get(&proposal.desk)
        .ok_or_else(|| Refusal::UnknownDesk { desk: proposal.desk.clone() })?;
    let collar_bps = mandate.price_collar_bps;

    let reference = match stop {
        Some(stop) => {
            let crossed = match proposal.side {
                Side::Buy => stop <= mark.price_scaled,
                Side::Sell => stop >= mark.price_scaled,
            };
            if crossed {
                return Err(Refusal::StopAlreadyCrossed { stop, mark: mark.price_scaled });
            }
            stop
        }
        None => mark.price_scaled,
    };
    let Some(limit) = proposal.price_scaled else { return Ok(()) };
    if crosses_beyond(proposal.side, limit, reference, collar_bps) {
        return Err(Refusal::OutsidePriceCollar { price: limit, mark: reference, collar_bps });
    }
    Ok(())
}

/// Compared as products, so no rounding moves the bound. Prices fit far below the overflow.
fn crosses_beyond(side: Side, limit: i128, reference: i128, collar_bps: u32) -> bool {
    let full = i128::from(WIDEST_COLLAR_BPS);
    let band = i128::from(collar_bps);
    match side {
        Side::Buy => limit.saturating_mul(full) > reference.saturating_mul(full + band),
        Side::Sell => limit.saturating_mul(full) < reference.saturating_mul(full - band),
    }
}
