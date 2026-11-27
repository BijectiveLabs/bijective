// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Releasing held proposals (decision 0013): each pass, every held proposal its parent's fills
//! made due goes through the same admission, collar and risk gates as a new proposal.
//!
//! Precondition: the pass recorded the fills it read before it runs. Postcondition: every due
//! release was placed, refused once and recorded, or left held for a stated reason (the trading
//! state, a flatten in force, a refusal earlier in this process, a leg waiting for its price).

use bijective_common::clock::Clock;
use bijective_model::{Proposal, Side};
use bijective_risk::desks::admit;

use crate::loop_::{Acted, Judgement, Reachable, place_proposal};
use crate::path::{OrderPath, PathError, Placement, enforces_reduce_only};

/// Places what is due of every held proposal. A refused release is not tried again in this
/// process, and the refusal reaches the strategy that made it.
pub async fn release_held(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    clock: &dyn Clock,
    judgement: &Judgement<'_>,
) -> Result<Vec<Acted>, PathError> {
    let mut acted = Vec::new();
    for due in path.releasable() {
        let id = due.proposal.client_id.clone();
        if path.release_refused(&id)
            || !path.may_release(due.proposal.reduce_only)
            || awaits_its_price(reachable, judgement, &due.proposal, clock.now_ns())
        {
            continue;
        }
        let at = due.from_position;
        let placement =
            place_proposal(path, reachable, clock, judgement, at, &due.proposal).await?;
        if matches!(placement, Placement::Refused { .. }) {
            path.note_release_refused(&id);
        }
        acted.push(Acted { position: at, placement });
    }
    Ok(acted)
}

/// Whether a group's limit leg stays with the platform until the mark reaches its price. Only
/// on a venue that does not enforce reduce-only: two legs resting there could both fill while
/// the node is down and turn the position over, so only the stop rests at the venue. Without a
/// fresh mark it waits.
fn awaits_its_price(
    reachable: &Reachable<'_>,
    judgement: &Judgement<'_>,
    proposal: &Proposal,
    now_ns: i128,
) -> bool {
    let (Some(_), Some(limit), None) =
        (proposal.oco.as_ref(), proposal.price_scaled, proposal.terms.stop_price_scaled)
    else {
        return false;
    };
    let order_id = crate::order_id::client_order_id(now_ns, 0);
    // A proposal admission refuses is released, so the refusal is recorded in its own words.
    let Ok(admitted) = admit(judgement.mandates, &reachable.declarations, proposal, &order_id)
    else {
        return false;
    };
    let enforced = reachable
        .declarations
        .get(&admitted.venue)
        .is_some_and(|d| enforces_reduce_only(d.reduce_only()));
    if enforced {
        return false;
    }
    let fresh = judgement
        .marks
        .get(&proposal.instrument)
        .filter(|m| now_ns - m.observed_at_ns <= judgement.staleness_bound_ns);
    fresh.is_none_or(|mark| match proposal.side {
        Side::Sell => mark.price_scaled < limit,
        Side::Buy => mark.price_scaled > limit,
    })
}
