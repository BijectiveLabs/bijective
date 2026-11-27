// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order lifecycle as the control stream records it, rebuilt at start.
//!
//! The order path writes an intent before the venue call and the venue's answer after it. This
//! projection folds both into the lifecycle state machine, so a restart knows which orders may
//! still execute and which are unknown. What the stream records and the state machine refuses is
//! kept as a finding and counted, not dropped: a contradiction in the record is for an operator.

use std::collections::{BTreeMap, BTreeSet};

use bijective_authority::Action;
use bijective_event_store::projection::Projection;
use bijective_lifecycle::{
    Fee, Ledger, LedgerChange, LedgerSummary, Order, OrderEvent, OrderState, PositionCause, Verdict,
};
use bijective_model::{InstrumentId, OrderRequest, Owner, Side, VenueSymbol};
use bijective_risk::valuation::attribution::Placed;
use bijective_venue::contract::Recorded;
use bijective_venue::normalise::{Rejection, VenueOutcome};

use crate::wire::{Adjustment, ControlEvent, EvidenceRead, decode, flatten_decision_id};

#[derive(Debug)]
pub enum OrdersError {
    Decode(bijective_serialization::codec::DecodeError),
}

impl std::fmt::Display for OrdersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrdersError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

impl std::error::Error for OrdersError {}

/// A terminal order, for returning what its reservation holds beyond its filled share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub decision_id: String,
    pub ordered_scaled: i128,
    pub filled_scaled: i128,
    /// The order's instrument is flat on its venue and no order there can still execute: the
    /// exposure its fills reserved for is gone.
    pub position_closed: bool,
}

/// Instruments flat on this ledger with nothing that can still move them: an unknown order
/// might have filled. One pass over the orders, so the sweep stays linear in their number.
fn closed_instruments(ledger: &Ledger) -> BTreeSet<bijective_model::VenueSymbol> {
    let mut live = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for o in ledger.orders() {
        seen.insert(o.request.instrument.clone());
        if !o.state.is_terminal() {
            live.insert(o.request.instrument.clone());
        }
    }
    seen.into_iter()
        .filter(|i| !live.contains(i))
        .filter(|i| ledger.position(i).is_none_or(|p| p.quantity_scaled() == 0))
        .collect()
}

/// The key of an intent recorded before intents named their venue. No venue can reconcile it.
pub const UNROUTED: &str = "";

/// Profit a fill realised by closing part of a position, as the ledger computed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Realized {
    pub venue: String,
    pub instrument: VenueSymbol,
    /// Signed, at the instrument's price scale plus its quantity scale.
    pub realized_scaled: i128,
    /// When the fill was recorded, by the clock that wrote it.
    pub at_ns: i128,
}

/// The fee a fill stated, or that it stated none, as the ledger applied it. Every fill pays one,
/// opening or closing, so it is kept apart from what a fill realised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charged {
    pub venue: String,
    pub instrument: VenueSymbol,
    pub fee: Option<Fee>,
    pub at_ns: i128,
}

/// A charge to a venue account that names no order, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCharged {
    pub venue: String,
    pub fee: Fee,
    pub at_ns: i128,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Orders {
    /// One ledger per venue: instrument symbols are not unique across venues, so one ledger would
    /// add two venues' positions under one key.
    ledgers: BTreeMap<String, Ledger>,
    venue_of: BTreeMap<String, String>,
    /// Who each order was placed for, where its intent said.
    owner_of: BTreeMap<String, Owner>,
    /// Adjustments that named an owner and that the ledger applied.
    owned_adjustments: Vec<Adjustment>,
    decision_of: BTreeMap<String, String>,
    first_intent_ns: BTreeMap<String, i128>,
    /// When each venue's latest pass that read its evidence was aligned: every fill it reported
    /// up to then is explained, so a later read need not reach further back.
    aligned_at_ns: BTreeMap<String, i128>,
    /// When each order's intent was recorded: the instant its submit was made at.
    intent_at: BTreeMap<String, i128>,
    /// The price and quantity scales last recorded per venue and instrument.
    scales: BTreeMap<(String, VenueSymbol), Declared>,
    verdicts: BTreeMap<String, Verdict>,
    /// Venues whose latest evidence read failed: nothing is known of what they hold.
    unread: BTreeSet<String>,
    findings: Vec<String>,
    realized: Vec<Realized>,
    charged: Vec<Charged>,
    account_charged: Vec<AccountCharged>,
    /// Each venue's charge identifiers already recorded.
    charge_ids: BTreeSet<(String, String)>,
    /// Orders an operator ordered cancelled: every order not yet finished when a cancel-orders
    /// command was carried out. Read from the record, so a restart still cancels them.
    cancel_ordered: BTreeSet<String>,
    /// The flatten command in force: from its carried-out record until a resume carried out
    /// after it. While it is in force every pass closes what the record still holds.
    flatten_asked: Option<String>,
}

impl Orders {
    /// Profit realised by fills recorded at or after `from_ns`, oldest first.
    pub fn realized_since(&self, from_ns: i128) -> impl Iterator<Item = &Realized> {
        self.realized.iter().filter(move |r| r.at_ns >= from_ns)
    }

    /// The fees of fills recorded at or after `from_ns`, oldest first. A fill delivered twice is
    /// charged once, because the ledger applies it once.
    pub fn charged_since(&self, from_ns: i128) -> impl Iterator<Item = &Charged> {
        self.charged.iter().filter(move |c| c.at_ns >= from_ns)
    }

    /// Account charges recorded at or after `from_ns`, oldest first.
    pub fn account_charged_since(&self, from_ns: i128) -> impl Iterator<Item = &AccountCharged> {
        self.account_charged.iter().filter(move |c| c.at_ns >= from_ns)
    }

    /// Whether `venue`'s charge `charge_id` is already recorded.
    #[must_use]
    pub fn has_charge(&self, venue: &str, charge_id: &str) -> bool {
        self.charge_ids.contains(&(venue.to_string(), charge_id.to_string()))
    }

    /// The order placed under `client_id`, on whichever venue it was routed to.
    pub fn order(&self, client_id: &str) -> Option<&Order> {
        self.ledgers.get(self.venue_of.get(client_id)?)?.order(client_id)
    }

    /// Every order routed to a venue, with who it was placed for, what its fills hold and what
    /// it could still fill, and every adjustment that named an owner, as a filled quantity, the instrument named by `identity` from its venue and symbol. `Err`
    /// names the first symbol `identity` cannot name: a figure built without that order would
    /// understate whoever placed it.
    pub fn placed(
        &self,
        identity: impl Fn(&str, &VenueSymbol) -> Option<InstrumentId>,
    ) -> Result<Vec<Placed>, (String, VenueSymbol)> {
        let mut placed = Vec::new();
        for (venue, ledger) in self.ledgers.iter().filter(|(v, _)| v.as_str() != UNROUTED) {
            for order in ledger.orders() {
                let symbol = &order.request.instrument;
                let instrument =
                    identity(venue, symbol).ok_or_else(|| (venue.clone(), symbol.clone()))?;
                let open = if order.state.is_terminal() { 0 } else { order.remaining_scaled() };
                let (filled_scaled, open_buy_scaled, open_sell_scaled) = match order.request.side {
                    Side::Buy => (order.filled_quantity_scaled, open, 0),
                    Side::Sell => (-order.filled_quantity_scaled, 0, open),
                };
                placed.push(Placed {
                    owner: self.owner_of.get(order.client_id()).cloned(),
                    venue: venue.clone(),
                    instrument,
                    filled_scaled,
                    open_buy_scaled,
                    open_sell_scaled,
                });
            }
        }
        // An adjustment that named its owner moves that owner's position, as a fill would.
        for a in &self.owned_adjustments {
            let instrument = identity(&a.venue, &a.instrument)
                .ok_or_else(|| (a.venue.clone(), a.instrument.clone()))?;
            let filled_scaled = match a.side {
                Side::Buy => a.quantity_scaled,
                Side::Sell => -a.quantity_scaled,
            };
            placed.push(Placed {
                owner: a.owner.clone(),
                venue: a.venue.clone(),
                instrument,
                filled_scaled,
                open_buy_scaled: 0,
                open_sell_scaled: 0,
            });
        }
        Ok(placed)
    }

    /// The orders and positions routed to `venue`. Empty for a venue that was never routed to.
    pub fn ledger_for(&self, venue: &str) -> Ledger {
        self.ledgers.get(venue).cloned().unwrap_or_default()
    }

    /// When the first intent routed to `venue` was written: evidence older than it names no order
    /// of this platform there.
    pub fn first_intent_ns(&self, venue: &str) -> Option<i128> {
        self.first_intent_ns.get(venue).copied()
    }

    /// The instant evidence for `venue` must reach back to, before any clock margin: its latest
    /// aligned pass, or its first intent where none is later. An aligned pass found every order
    /// it was shown explained by its fills, so nothing before it is still owed. `None` when
    /// nothing was routed there.
    pub fn evidence_watermark_ns(&self, venue: &str) -> Option<i128> {
        let first = self.first_intent_ns(venue)?;
        Some(self.aligned_at_ns.get(venue).map_or(first, |aligned| first.max(*aligned)))
    }

    /// Every order routed to `venue`, as a venue adapter takes it back after a restart.
    pub fn recorded_for(&self, venue: &str) -> Vec<Recorded> {
        let Some(ledger) = self.ledgers.get(venue).filter(|_| venue != UNROUTED) else {
            return Vec::new();
        };
        ledger
            .orders()
            .filter_map(|o| {
                Some(Recorded {
                    request: o.request.clone(),
                    intent_ns: *self.intent_at.get(o.client_id())?,
                    venue_order_id: o.venue_order_id.clone(),
                    terminal: o.state.is_terminal(),
                })
            })
            .collect()
    }

    /// Orders a cancel can still reach: accepted or part filled, on a venue they were routed to.
    /// One already asked to cancel is pending, not working.
    pub fn working(&self) -> impl Iterator<Item = (&str, &Order)> {
        self.ledgers.iter().filter(|(venue, _)| venue.as_str() != UNROUTED).flat_map(
            |(venue, ledger)| {
                ledger
                    .orders()
                    .filter(|o| {
                        matches!(o.state, OrderState::Accepted | OrderState::PartiallyFilled)
                    })
                    .map(move |o| (venue.as_str(), o))
            },
        )
    }

    /// Counts summed over every venue.
    pub fn summary(&self) -> LedgerSummary {
        self.ledgers.values().map(Ledger::summary).fold(LedgerSummary::default(), |a, b| {
            LedgerSummary {
                orders: a.orders + b.orders,
                may_execute: a.may_execute + b.may_execute,
                unknown: a.unknown + b.unknown,
                positions_open: a.positions_open + b.positions_open,
            }
        })
    }

    /// Venues a reconciliation pass could still settle something on: an order there may still
    /// execute (working, unknown or reconciling), so a fill there is read within the unsettled
    /// cadence rather than the position check's, its last pass diverged, or its last read failed. Intents recorded without a venue are not among them.
    /// No venue can answer for those.
    pub fn unsettled(&self) -> Vec<String> {
        let mut venues: Vec<String> = self
            .ledgers
            .iter()
            .filter(|(venue, ledger)| {
                venue.as_str() != UNROUTED && ledger.summary().may_execute > 0
            })
            .map(|(venue, _)| venue.clone())
            .collect();
        venues.extend(self.diverged().map(str::to_string));
        venues.extend(self.unread.iter().cloned());
        venues.sort_unstable();
        venues.dedup();
        venues
    }

    /// The price and quantity scales the record last declared for `instrument` on `venue`.
    #[must_use]
    pub fn scales(&self, venue: &str, instrument: &VenueSymbol) -> Option<(u8, u8)> {
        self.declared(venue, instrument).map(|d| (d.price_scale, d.quantity_scale))
    }

    /// What the record last declared about `instrument` on `venue`.
    #[must_use]
    pub fn declared(&self, venue: &str, instrument: &VenueSymbol) -> Option<&Declared> {
        self.scales.get(&(venue.to_string(), instrument.clone()))
    }

    /// The flatten command still to act on, by its identifier.
    #[must_use]
    pub fn flatten_asked(&self) -> Option<&str> {
        self.flatten_asked.as_deref()
    }

    /// Whether `client_id` is an order the flatten in force placed to close a position.
    #[must_use]
    pub fn is_closing_for_flatten(&self, client_id: &str) -> bool {
        let Some(command_id) = self.flatten_asked.as_deref() else { return false };
        self.decision_of.get(client_id).is_some_and(|d| *d == flatten_decision_id(command_id))
    }

    /// Every venue's ledger, in venue order.
    pub fn ledgers(&self) -> impl Iterator<Item = (&str, &Ledger)> {
        self.ledgers.iter().map(|(venue, ledger)| (venue.as_str(), ledger))
    }

    /// Whether a carried-out cancel-orders command named this order.
    pub fn is_ordered_cancelled(&self, client_id: &str) -> bool {
        self.cancel_ordered.contains(client_id)
    }

    fn declared_scales(&mut self, event: &ControlEvent) {
        if let ControlEvent::InstrumentScales {
            venue,
            instrument,
            price_scale,
            quantity_scale,
            identity,
            quote_currency,
            ..
        } = event
        {
            let key = (venue.clone(), instrument.clone());
            let declared = Declared {
                price_scale: *price_scale,
                quantity_scale: *quantity_scale,
                identity: identity.clone(),
                quote_currency: quote_currency.clone(),
            };
            self.scales.insert(key, declared);
        }
    }

    /// Once per venue and charge identifier: a charge read twice is one charge.
    fn charged(&mut self, venue: &str, charge: &bijective_lifecycle::AccountCharge, at_ns: i128) {
        if self.charge_ids.insert((venue.to_string(), charge.charge_id.clone())) {
            let (venue, fee) = (venue.to_string(), charge.fee.clone());
            self.account_charged.push(AccountCharged { venue, fee, at_ns });
        }
    }

    fn order_cancelled(&mut self) {
        for ledger in self.ledgers.values() {
            let open = ledger.orders().filter(|o| !o.state.is_terminal());
            self.cancel_ordered.extend(open.map(|o| o.client_id().to_string()));
        }
    }

    /// Venues whose latest evidence read failed.
    pub fn unread(&self) -> impl Iterator<Item = &str> {
        self.unread.iter().map(String::as_str)
    }

    /// Venues whose reconciliation diverged and has not since aligned.
    pub fn diverged(&self) -> impl Iterator<Item = &str> {
        self.verdicts.iter().filter(|(_, v)| **v == Verdict::Diverged).map(|(k, _)| k.as_str())
    }

    /// Each a sentence naming the order and what the state machine refused.
    pub fn findings(&self) -> &[String] {
        &self.findings
    }

    /// Every order in a terminal state, with the decision that reserved budget for it, what was
    /// ordered and what filled. A finished order holds budget only for its filled share.
    pub fn finished(&self) -> Vec<Finished> {
        let mut out = Vec::new();
        for ledger in self.ledgers.values() {
            let closed = closed_instruments(ledger);
            for o in ledger.orders().filter(|o| o.state.is_terminal()) {
                let Some(decision_id) = self.decision_of.get(o.client_id()) else { continue };
                out.push(Finished {
                    decision_id: decision_id.clone(),
                    ordered_scaled: o.request.quantity_scaled,
                    filled_scaled: o.filled_quantity_scaled,
                    position_closed: closed.contains(&o.request.instrument),
                });
            }
        }
        out
    }

    fn intent(&mut self, venue: Option<&str>, request: &OrderRequest, at_ns: i128) {
        if self.venue_of.contains_key(&request.client_id) {
            self.findings.push(format!("{} was placed twice", request.client_id));
            return;
        }
        let venue = venue.unwrap_or(UNROUTED).to_string();
        let ledger = self.ledgers.entry(venue.clone()).or_default();
        match ledger.create(request.clone()) {
            Ok(()) => {
                self.venue_of.insert(request.client_id.clone(), venue.clone());
                self.intent_at.insert(request.client_id.clone(), at_ns);
                self.first_intent_ns.entry(venue).or_insert(at_ns);
                self.apply_to(&request.client_id, &OrderEvent::Submitted, at_ns);
            }
            Err(e) => self.findings.push(e.to_string()),
        }
    }

    fn adjust(&mut self, a: &Adjustment) {
        let ledger = self.ledgers.entry(a.venue.clone()).or_default();
        match ledger.adjust(&a.instrument, cause_of(a), a.side, a.price_scaled, a.quantity_scaled) {
            Ok(_) if a.owner.is_some() => self.owned_adjustments.push(a.clone()),
            Ok(_) => {}
            Err(e) => self.findings.push(format!("adjustment {}: {e}", a.adjustment_id)),
        }
    }

    fn apply_to(&mut self, client_id: &str, event: &OrderEvent, at_ns: i128) {
        let Some(venue) = self.venue_of.get(client_id).cloned() else {
            self.findings.push(format!("{client_id} has no intent before its {}", event.name()));
            return;
        };
        let Some(ledger) = self.ledgers.get_mut(&venue) else { return };
        match ledger.apply(client_id, event, at_ns) {
            Ok(LedgerChange::Applied { position, .. }) => {
                if let (OrderEvent::Filled(fill), Some(order)) = (event, ledger.order(client_id)) {
                    self.charged.push(Charged {
                        venue: venue.clone(),
                        instrument: order.request.instrument.clone(),
                        fee: fill.fee.clone(),
                        at_ns,
                    });
                }
                if let Some(change) = position.filter(|c| c.realized_scaled != 0) {
                    self.realized.push(Realized {
                        venue,
                        instrument: change.instrument,
                        realized_scaled: change.realized_scaled,
                        at_ns,
                    });
                }
            }
            Ok(LedgerChange::Duplicate) => {}
            Err(e) => self.findings.push(e.to_string()),
        }
    }
}

/// The position record names the command and its reason, so an auditor reads why it moved.
pub fn cause_of(a: &Adjustment) -> PositionCause {
    PositionCause::Adjustment { adjustment_id: a.adjustment_id.clone(), reason: a.reason.clone() }
}

/// What a submit's answer means for the order. A duplicate says the venue already holds it.
fn submit_answer(outcome: &VenueOutcome) -> OrderEvent {
    match outcome {
        VenueOutcome::Accepted | VenueOutcome::Rejected(Rejection::Duplicate) => {
            OrderEvent::Accepted { venue_order_id: None }
        }
        VenueOutcome::Rejected(r) => OrderEvent::Rejected { reason: format!("{r:?}") },
        VenueOutcome::NotSent => OrderEvent::NotSent,
        VenueOutcome::Unknown(u) => OrderEvent::OutcomeUnknown { detail: format!("{u:?}") },
    }
}

/// What a cancel's answer means. An accepted or unknown cancel leaves it pending: only the venue's
/// later evidence says the order stopped working.
fn cancel_answer(outcome: &VenueOutcome) -> Option<OrderEvent> {
    match outcome {
        VenueOutcome::Accepted | VenueOutcome::Unknown(_) => None,
        VenueOutcome::Rejected(r) => Some(OrderEvent::CancelRejected { reason: format!("{r:?}") }),
        VenueOutcome::NotSent => Some(OrderEvent::CancelRejected { reason: "not sent".into() }),
    }
}

impl Projection for Orders {
    type Event = ControlEvent;
    type Error = OrdersError;

    fn decode(payload: &[u8]) -> Result<ControlEvent, OrdersError> {
        decode(payload).map_err(OrdersError::Decode)
    }

    fn apply(&mut self, event: &ControlEvent) -> Result<(), OrdersError> {
        match event {
            ControlEvent::Intent { decision_id, venue, owner, request, at_ns, .. } => {
                self.intent(venue.as_deref(), request, *at_ns);
                if let Some(owner) = owner {
                    self.owner_of.entry(request.client_id.clone()).or_insert(owner.clone());
                }
                self.decision_of.entry(request.client_id.clone()).or_insert(decision_id.clone());
            }
            ControlEvent::Reconciled { venue, client_id, settlement, at_ns } => {
                if self.venue_of.get(client_id).is_some_and(|v| v != venue) {
                    self.findings.push(format!("{client_id} was reconciled against {venue}"));
                    return Ok(());
                }
                self.apply_to(client_id, &settlement.event(), *at_ns);
            }
            // Only agreement clears a divergence: a pass that could not see everything has not
            // shown the divergence is gone.
            ControlEvent::PositionAdjusted { adjustment, .. } => self.adjust(adjustment),
            ControlEvent::ReconciliationCompleted { venue, verdict, evidence, at_ns, .. } => {
                if *verdict == Verdict::Aligned && *evidence == EvidenceRead::Read {
                    self.aligned_at_ns.insert(venue.clone(), *at_ns);
                }
                if *evidence == EvidenceRead::Failed {
                    self.unread.insert(venue.clone());
                } else {
                    self.unread.remove(venue);
                }
                let held = self.verdicts.get(venue).copied();
                if !(held == Some(Verdict::Diverged) && *verdict == Verdict::Unverified) {
                    self.verdicts.insert(venue.clone(), *verdict);
                }
            }
            ControlEvent::Settled { client_id, outcome, at_ns } => {
                self.apply_to(client_id, &submit_answer(outcome), *at_ns);
            }
            ControlEvent::Filled { client_id, fill, at_ns } => {
                self.apply_to(client_id, &OrderEvent::Filled(fill.clone()), *at_ns);
            }
            ControlEvent::InstrumentScales { .. } => self.declared_scales(event),
            ControlEvent::AccountCharged { venue, charge, at_ns } => {
                self.charged(venue, charge, *at_ns);
            }
            // A refused command carries its reason in the detail. Only one carried out names orders.
            ControlEvent::CommandApplied { action: Action::CancelOrders, detail, .. }
                if detail.is_empty() =>
            {
                self.order_cancelled();
            }
            // Every open order is ordered cancelled, stops and exits too: the positions are
            // closed at the market instead.
            ControlEvent::CommandApplied {
                action: Action::Flatten, detail, command_id, ..
            } if detail.is_empty() => {
                self.order_cancelled();
                self.flatten_asked = Some(command_id.clone());
            }
            ControlEvent::CommandApplied { action: Action::Resume, detail, .. }
                if detail.is_empty() =>
            {
                self.flatten_asked = None;
            }
            ControlEvent::Withdrawn { client_id, outcome, at_ns, .. } => {
                self.apply_to(client_id, &OrderEvent::CancelRequested, *at_ns);
                if let Some(answer) = cancel_answer(outcome) {
                    self.apply_to(client_id, &answer, *at_ns);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// What an `InstrumentScales` entry declares about one instrument on one venue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    pub price_scale: u8,
    pub quantity_scale: u8,
    pub identity: Option<String>,
    pub quote_currency: Option<String>,
}

#[cfg(test)]
#[path = "../tests/unit/orders_tests.rs"]
mod tests;
