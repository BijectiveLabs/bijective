// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Whether a reduce-only order reduces: judged on the venue's ledger, the platform's record of
//! what it holds there, before any budget is drawn or the venue is called.

use bijective_lifecycle::Ledger;
use bijective_model::{OrderRequest, Side};
use bijective_venue::profile::{CapabilityDeclaration, ReduceOnly};

use super::OrderPath;
use crate::wire::RefusalReason;

impl OrderPath {
    /// `None` when `request` only moves the held position toward flat, counting the reduce-only
    /// orders already working the same way. Otherwise the refusal with both figures. On a venue
    /// that enforces reduce-only, an order an operator ordered cancelled is not counted: the venue
    /// cannot let it and this one together pass through flat. Orders of the decision's group peers
    /// (decision 0013) are alternatives to it and are not counted.
    pub(super) fn does_not_reduce(
        &self,
        declaration: &CapabilityDeclaration,
        venue: &str,
        request: &OrderRequest,
        decision_id: &str,
    ) -> Option<RefusalReason> {
        let orders = self.replayed.state().map(|r| r.orders());
        let ledger = orders.map(|o| o.ledger_for(venue)).unwrap_or_default();
        let enforced = enforces_reduce_only(declaration.reduce_only());
        let peers = self.peer_orders(decision_id);
        let counted = |client_id: &str| {
            !(enforced && orders.is_some_and(|o| o.is_ordered_cancelled(client_id)))
                && !peers.contains(client_id)
        };
        judge(&ledger, request, &counted)
    }

    /// `None` unless `request` sells an instrument that cannot be held short beyond what the
    /// record holds on `venue`, counting every sell there that may still execute. The record, not
    /// the venue's account, is the bound: a holding the platform did not place is sold only once
    /// a risk officer has adjusted it in. Group peers' orders are not counted, as above.
    pub(super) fn uncovered(
        &self,
        declaration: &CapabilityDeclaration,
        venue: &str,
        request: &OrderRequest,
        decision_id: &str,
    ) -> Option<RefusalReason> {
        if request.side != Side::Sell {
            return None;
        }
        let kind = declaration.instruments().get(&request.instrument).ok()?.kind;
        if kind.can_be_short() {
            return None;
        }
        let ledger =
            self.replayed.state().map(|r| r.orders().ledger_for(venue)).unwrap_or_default();
        covered(&ledger, request, &self.peer_orders(decision_id))
    }
}

/// The held quantity must cover this sell and every other sell of the instrument still working.
fn covered(
    ledger: &Ledger,
    request: &OrderRequest,
    peers: &std::collections::BTreeSet<String>,
) -> Option<RefusalReason> {
    let held = ledger.position(&request.instrument).map_or(0, |p| p.quantity_scaled());
    let working: i128 = ledger
        .orders()
        .filter(|o| o.request.side == Side::Sell && o.state.may_execute())
        .filter(|o| {
            o.request.instrument == request.instrument && o.client_id() != request.client_id
        })
        .filter(|o| !peers.contains(o.client_id()))
        .map(|o| o.request.quantity_scaled - o.filled_quantity_scaled)
        .sum();
    let left = held.saturating_sub(working).saturating_sub(request.quantity_scaled);
    if left >= 0 {
        None
    } else {
        Some(RefusalReason::Uncovered { held_scaled: held, working_scaled: working })
    }
}

fn signed(side: Side, quantity_scaled: i128) -> i128 {
    match side {
        Side::Buy => quantity_scaled,
        Side::Sell => -quantity_scaled,
    }
}

/// A reduction opposes the held position and, with what is already working toward flat, does not
/// pass through it: crossing flat would open the other side.
/// Whether a venue refuses or clamps a reduce-only order that would pass through flat.
#[must_use]
pub fn enforces_reduce_only(guarantee: ReduceOnly) -> bool {
    matches!(guarantee, ReduceOnly::PlacementPrecondition | ReduceOnly::FillTimeClamp)
}

fn judge(
    ledger: &Ledger,
    request: &OrderRequest,
    counted: &dyn Fn(&str) -> bool,
) -> Option<RefusalReason> {
    let held = ledger.position(&request.instrument).map_or(0, |p| p.quantity_scaled());
    let asked = signed(request.side, request.quantity_scaled);
    let working: i128 = ledger
        .orders()
        .filter(|o| o.request.reduce_only && o.state.may_execute() && counted(o.client_id()))
        .filter(|o| {
            o.request.instrument == request.instrument && o.client_id() != request.client_id
        })
        .map(|o| signed(o.request.side, o.request.quantity_scaled - o.filled_quantity_scaled))
        .filter(|w| w.signum() == asked.signum())
        .sum();
    let opposes = held != 0 && held.signum() != asked.signum();
    let within =
        working.saturating_add(asked).checked_abs().is_some_and(|total| total <= held.abs());
    if opposes && within {
        None
    } else {
        Some(RefusalReason::DoesNotReduce { held_scaled: held, working_scaled: working })
    }
}
