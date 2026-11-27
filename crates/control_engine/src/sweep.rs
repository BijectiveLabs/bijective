// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Withdrawing resting orders when the node is not active. The policy is in
//! `docs/concepts/order_path.md`.
//!
//! Precondition: the caller passes the venues it can reach. Postcondition: every selected order on
//! a reachable venue has had a cancel asked for and recorded. The order stays pending until the
//! venue's evidence says it stopped. An order on an unreachable venue is reported, never skipped.

use std::collections::{BTreeMap, BTreeSet};

use bijective_limits::SafeState;
use bijective_model::VenueSymbol;
use bijective_venue::normalise::VenueOutcome;
use bijective_venue::profile::ReduceOnly;

use crate::loop_::Reachable;
use crate::orders::Orders;
use crate::path::{OrderPath, PathError};

/// A working order selected for withdrawal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resting {
    pub venue: String,
    pub instrument: VenueSymbol,
    pub client_id: String,
}

/// What one sweep did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Swept {
    /// Each order a cancel was asked for, with the venue's answer.
    pub withdrawn: Vec<(String, VenueOutcome)>,
    /// Selected orders on a venue this node cannot reach: still working, for an operator.
    pub unreachable: Vec<String>,
}

/// The working orders to withdraw in `state`: those an operator ordered cancelled or a fill on a
/// group peer displaced (`displaced`, decision 0013) or whose expiry has passed at `now_ns`
/// (decision 0014), in any state, and, when the node is not active, those that can add exposure.
/// A closing order of the flatten in force is kept on a trusted venue, enforced or not (decision
/// 0010). Pure: the same inputs always select the same orders.
#[must_use]
pub fn resting_to_withdraw(
    orders: &Orders,
    state: SafeState,
    guarantees: &BTreeMap<String, ReduceOnly>,
    displaced: &BTreeSet<String>,
    now_ns: i128,
) -> Vec<Resting> {
    let active = matches!(state, SafeState::Active);
    let untrusted: BTreeSet<&str> = orders.unread().chain(orders.diverged()).collect();
    orders
        .working()
        .filter(|(venue, order)| {
            let expired = order.request.terms.expire_at_ns.is_some_and(|at| at <= now_ns);
            if expired
                || orders.is_ordered_cancelled(order.client_id())
                || displaced.contains(order.client_id())
            {
                return true;
            }
            let enforced = matches!(
                guarantees.get(*venue),
                Some(ReduceOnly::PlacementPrecondition | ReduceOnly::FillTimeClamp)
            );
            let trusted = !untrusted.contains(venue);
            // The flatten's own closing order is what a non-active node is for: withdrawing it
            // where reduce-only is not enforced would only race its fill and place it again.
            let closing = orders.is_closing_for_flatten(order.client_id());
            !active && !((order.request.reduce_only && (enforced || closing)) && trusted)
        })
        .map(|(venue, order)| Resting {
            venue: venue.to_string(),
            instrument: order.request.instrument.clone(),
            client_id: order.client_id().to_string(),
        })
        .collect()
}

/// Asks every selected order's venue to cancel it. Cancel is never refused clearance, so a
/// breached or halted node can still do this.
pub async fn withdraw_resting(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    at_ns: i128,
) -> Result<Swept, PathError> {
    let guarantees: BTreeMap<String, ReduceOnly> = reachable
        .declarations
        .iter()
        .map(|(venue, declaration)| (venue.clone(), declaration.reduce_only()))
        .collect();
    let displaced = path.displaced_orders();
    let selected = path
        .recorded_orders()
        .map(|orders| {
            resting_to_withdraw(orders, path.trading_state(), &guarantees, &displaced, at_ns)
        })
        .unwrap_or_default();

    let mut swept = Swept::default();
    for resting in selected {
        let Some(adapter) = reachable.adapters.get_mut(&resting.venue) else {
            swept.unreachable.push(resting.client_id);
            continue;
        };
        let withdrawal =
            path.withdraw(&mut **adapter, &resting.instrument, &resting.client_id, at_ns).await?;
        swept.withdrawn.push((resting.client_id, withdrawal.outcome));
    }
    Ok(swept)
}

#[cfg(test)]
#[path = "../tests/unit/sweep_tests.rs"]
mod tests;
