// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A flatten command carried out: every position the record holds, closed by one reduce-only
//! market order on its venue (decision 0010).
//!
//! Precondition: the pass carried out its commands and its sweep first. Postcondition: while a
//! flatten is in force, every open position on a trusted, reachable venue that its working orders
//! do not already close has had one order submitted, or refused once, and recorded. A venue whose
//! record is not trusted or cannot be reached is reported.

use bijective_common::clock::Clock;
use bijective_lifecycle::Ledger;
use bijective_model::{OrderRequest, Side, VenueSymbol};

use crate::loop_::Reachable;
use crate::order_id::client_order_id;
use crate::orders::Orders;
use crate::path::{FlattenOrder, OrderPath, PathError, Placement};
use crate::wire::flatten_decision_id;

/// What one pass did for a flatten.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Flattened {
    /// Each order's client identifier with what came of it.
    pub placed: Vec<(String, Placement)>,
    /// Venues whose record is not trusted (the last read failed or diverged): what they hold is
    /// not known well enough to close by quantity, so an operator closes it at the venue.
    pub untrusted: Vec<String>,
    /// Venues with positions this node cannot reach.
    pub unreachable: Vec<String>,
}

/// One position to close, by the quantity left once the reduce-only orders already working
/// toward flat are counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closing {
    pub venue: String,
    pub instrument: VenueSymbol,
    pub side: Side,
    pub quantity_scaled: i128,
}

/// Every position the record holds and what closes it, with the venues left out as untrusted.
/// A reduce-only order still working counts toward flat, except one ordered cancelled on a venue
/// in `enforced`, which enforces reduce-only. Pure: the same record always gives the same orders.
#[must_use]
pub fn closing_orders(orders: &Orders, enforced: &[String]) -> (Vec<Closing>, Vec<String>) {
    let untrusted: Vec<String> =
        orders.unread().chain(orders.diverged()).map(str::to_string).collect();
    let mut closing = Vec::new();
    for (venue, ledger) in orders.ledgers() {
        if untrusted.iter().any(|u| u == venue) {
            continue;
        }
        for (instrument, held) in ledger.open_positions() {
            let cancelled =
                |id: &str| enforced.iter().any(|e| e == venue) && orders.is_ordered_cancelled(id);
            let left = held.abs() - working_toward_flat(ledger, instrument, held, &cancelled);
            if left <= 0 {
                continue;
            }
            let side = if held > 0 { Side::Sell } else { Side::Buy };
            let (venue, instrument) = (venue.to_string(), instrument.clone());
            closing.push(Closing { venue, instrument, side, quantity_scaled: left });
        }
    }
    (closing, untrusted)
}

/// The unfilled quantity of reduce-only orders that may still execute against `held`.
fn working_toward_flat(
    ledger: &Ledger,
    instrument: &VenueSymbol,
    held: i128,
    cancelled: &dyn Fn(&str) -> bool,
) -> i128 {
    let closes = if held > 0 { Side::Sell } else { Side::Buy };
    ledger
        .orders()
        .filter(|o| o.request.reduce_only && o.state.may_execute() && !cancelled(o.client_id()))
        .filter(|o| &o.request.instrument == instrument && o.request.side == closes)
        .map(|o| o.request.quantity_scaled - o.filled_quantity_scaled)
        .sum()
}

/// Places the orders a flatten in force still needs. Nothing when none is in force.
pub async fn flatten_held(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    clock: &dyn Clock,
) -> Result<Flattened, PathError> {
    let mut done = Flattened::default();
    let Some(orders) = path.recorded_orders() else { return Ok(done) };
    let Some(command_id) = orders.flatten_asked() else { return Ok(done) };
    let decision_id = flatten_decision_id(command_id);
    let enforced: Vec<String> = reachable
        .declarations
        .iter()
        .filter(|(_, d)| crate::path::enforces_reduce_only(d.reduce_only()))
        .map(|(venue, _)| venue.clone())
        .collect();
    let (closing, untrusted) = closing_orders(orders, &enforced);
    done.untrusted = untrusted;

    for (index, close) in (0u64..).zip(closing) {
        if path.flatten_refused(&close.venue, &close.instrument) {
            continue;
        }
        let Some(adapter) = reachable.adapters.get_mut(&close.venue) else {
            done.unreachable.push(close.venue);
            continue;
        };
        let request = OrderRequest {
            client_id: client_order_id(clock.now_ns(), index),
            instrument: close.instrument,
            side: close.side,
            quantity_scaled: close.quantity_scaled,
            price_scaled: None,
            reduce_only: true,
            terms: Default::default(),
        };
        let order =
            FlattenOrder { decision_id: &decision_id, venue: &close.venue, request: &request };
        let placement = path.flatten_one(&mut **adapter, clock, &order).await?;
        done.placed.push((request.client_id, placement));
    }
    Ok(done)
}

#[cfg(test)]
#[path = "../tests/unit/flatten_tests.rs"]
mod tests;
