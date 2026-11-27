// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A simulated venue rebuilt from the record of the run it stands for, so a paper node that
//! restarts finds the venue as it left it: the same orders under the same identifiers, the same
//! fills, positions and cash, and its working orders resting again.
//!
//! The book is not restored: it is rebuilt from the market the node reads next. A stop with no
//! fill is held again until its price is reached, since the record does not say whether it had
//! triggered.

use bijective_lifecycle::{FillReport, Ledger, OrderEvent, OrderState};
use bijective_venue::profile::CapabilityDeclaration;

use super::{Command, Engine, Pending};
use crate::account::Terms;

/// What a restore took back, for the node to report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Restored {
    pub orders: usize,
    pub fills: usize,
    pub working: usize,
}

impl Engine {
    /// Takes back every order of `record` the venue had accepted, with its fills, at `now_ns`.
    /// Precondition: a fresh engine, before any order or market input. An order the record never
    /// saw accepted, or whose fate it does not know, is not taken back: reconciliation then reads
    /// it as absent, as it would at a venue that never held it.
    pub fn restore(
        &mut self,
        declaration: &CapabilityDeclaration,
        record: &Ledger,
        now_ns: i128,
    ) -> Restored {
        let mut restored = Restored::default();
        for order in record.orders() {
            if !reached_the_venue(order.state) {
                continue;
            }
            let request = order.request.clone();
            let client_id = request.client_id.clone();
            if let Err(e) = self.ledger.create(request.clone()) {
                self.faults.push(e.to_string());
                continue;
            }
            self.record(&client_id, &OrderEvent::Submitted, now_ns);
            let venue_order_id = order.venue_order_id.clone();
            self.next_order = self.next_order.max(sequence(venue_order_id.as_deref(), "sim-"));
            self.record(&client_id, &OrderEvent::Accepted { venue_order_id }, now_ns);
            if let Some(terms) = terms_of(declaration, &request) {
                self.terms.insert(client_id.clone(), terms);
            }
            for fill in &order.fills {
                self.take_back_fill(&request, fill, now_ns);
                restored.fills += 1;
            }
            restored.orders += 1;
            if self.rest_again(order, now_ns) {
                restored.working += 1;
            }
        }
        restored
    }

    fn take_back_fill(
        &mut self,
        request: &bijective_model::OrderRequest,
        fill: &bijective_lifecycle::Fill,
        now_ns: i128,
    ) {
        if let (Some(account), Some(terms)) =
            (self.account.as_mut(), self.terms.get(&request.client_id))
        {
            let filled = (fill.price_scaled, fill.quantity_scaled, fill.liquidity);
            account.fill(&request.client_id, terms, request.side, filled);
        }
        self.next_fill = self.next_fill.max(sequence(Some(&fill.fill_id), "sim-fill-"));
        self.record(&request.client_id, &OrderEvent::Filled(fill.clone()), now_ns);
        self.traded.insert(request.instrument.clone(), ());
        self.fills.push(FillReport {
            client_id: Some(request.client_id.clone()),
            instrument: request.instrument.clone(),
            fill: fill.clone(),
        });
    }

    /// Ends the order as the record ended it, or puts it back where it waited: a cancel the
    /// record asked for is carried out on the next settle, as a venue would still carry it out.
    fn rest_again(&mut self, order: &bijective_lifecycle::Order, now_ns: i128) -> bool {
        let client_id = order.client_id().to_string();
        match order.state {
            OrderState::Canceled => self.record(&client_id, &OrderEvent::Canceled, now_ns),
            OrderState::Expired => self.record(&client_id, &OrderEvent::Expired, now_ns),
            OrderState::CancelPending => {
                self.record(&client_id, &OrderEvent::CancelRequested, now_ns);
                let command = Command::Cancel;
                self.queue.push_back(Pending { client_id, command, effective_at_ns: now_ns });
            }
            OrderState::Accepted | OrderState::PartiallyFilled => {
                return self.working_again(order);
            }
            _ => {}
        }
        false
    }

    fn working_again(&mut self, order: &bijective_lifecycle::Order) -> bool {
        let request = &order.request;
        let client_id = request.client_id.clone();
        let held_stop = request.terms.stop_price_scaled.is_some() && order.fills.is_empty();
        if held_stop {
            self.stops.entry(request.instrument.clone()).or_default().push(client_id.clone());
        } else if request.price_scaled.is_some() && request.terms.time_in_force.may_rest() {
            self.resting.entry(request.instrument.clone()).or_default().push(client_id.clone());
        } else {
            return false;
        }
        self.reserve_remaining(order);
        true
    }

    /// A working buy holds back what its remainder may still cost, as at its submission.
    fn reserve_remaining(&mut self, order: &bijective_lifecycle::Order) {
        let request = &order.request;
        let price = request.price_scaled.or(request.terms.stop_price_scaled);
        let (Some(account), Some(terms), Some(price)) =
            (self.account.as_mut(), self.terms.get(&request.client_id), price)
        else {
            return;
        };
        let remaining = (price, order.remaining_scaled());
        if !account.reserve(&request.client_id, terms, request.side, remaining) {
            let detail = format!("{}: its remainder exceeds the restored cash", request.client_id);
            self.faults.push(detail);
        }
    }
}

/// Whether the venue held the order: it answered, and the record knows how it ended or that it
/// still works.
fn reached_the_venue(state: OrderState) -> bool {
    matches!(
        state,
        OrderState::Accepted
            | OrderState::PartiallyFilled
            | OrderState::CancelPending
            | OrderState::Filled
            | OrderState::Canceled
            | OrderState::Expired
    )
}

fn terms_of(
    declaration: &CapabilityDeclaration,
    request: &bijective_model::OrderRequest,
) -> Option<Terms> {
    let instrument = declaration.instruments().get(&request.instrument).ok()?;
    Some(Terms {
        currency: instrument.quote_currency.clone(),
        price_scale: instrument.precision.price_scale,
        quantity_scale: instrument.precision.quantity_scale,
    })
}

/// The number this engine gave an identifier (`sim-7`, `sim-fill-12`), so new ones follow it.
fn sequence(identifier: Option<&str>, prefix: &str) -> u64 {
    identifier.and_then(|i| i.strip_prefix(prefix)).and_then(|n| n.parse().ok()).unwrap_or(0)
}
