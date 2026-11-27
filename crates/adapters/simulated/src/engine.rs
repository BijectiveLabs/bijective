// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The matching engine. Each input runs in a fixed order: the market first updates the book and
//! fills orders already resting, then commands whose latency has elapsed are applied and matched,
//! so a resting order always sees new market state before an order submitted in reaction to it.
//!
//! The venue side keeps its own ledger through the platform's lifecycle, so a simulated order
//! obeys the same state machine as a live one, and its record is reported as venue evidence.

use std::collections::{BTreeMap, VecDeque};

use bijective_lifecycle::{
    FillReport, Ledger, Liquidity, OrderEvent, OrderState, OrderStatusReport, PositionEvidence,
    PositionReport, ReportedStatus, VenueEvidence,
};
use bijective_model::market::{BookSide, ExchangeTime, MarketRecord};
use bijective_model::{BookTop, OrderRequest, Side, TimeInForce, VenueSymbol};
use bijective_venue::normalise::{Rejection, VenueOutcome};
use bijective_venue::profile::{CapabilityDeclaration, InstrumentStatus};

use crate::account::{Account, Terms};
use crate::book::SimBook;
use crate::model::{FillModel, SimulationModel};

mod restore;
pub use restore::Restored;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Place,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    client_id: String,
    command: Command,
    effective_at_ns: i128,
}

pub struct Engine {
    model: SimulationModel,
    books: BTreeMap<VenueSymbol, SimBook>,
    ledger: Ledger,
    /// Resting orders per instrument, oldest first: time priority among our own orders.
    resting: BTreeMap<VenueSymbol, Vec<String>>,
    queue: VecDeque<Pending>,
    fills: Vec<FillReport>,
    traded: BTreeMap<VenueSymbol, ()>,
    next_order: u64,
    next_fill: u64,
    /// Anything the lifecycle refused inside the engine. Empty in a sound run. Never dropped.
    faults: Vec<String>,
    /// The sandbox account, where one is declared. Without one nothing is charged or refused for
    /// funds, as in a backtest.
    account: Option<Account>,
    /// What each order is priced in, kept from its submission for the fills that follow.
    terms: BTreeMap<String, Terms>,
    /// Stops placed and not yet triggered, per instrument, oldest first.
    stops: BTreeMap<VenueSymbol, Vec<String>>,
    /// Stops that have triggered, which are then placed as the order they carry.
    triggered: std::collections::BTreeSet<String>,
}

impl Engine {
    pub fn new(model: SimulationModel) -> Self {
        Self {
            model,
            books: BTreeMap::new(),
            ledger: Ledger::default(),
            resting: BTreeMap::new(),
            queue: VecDeque::new(),
            fills: Vec::new(),
            traded: BTreeMap::new(),
            next_order: 0,
            next_fill: 0,
            faults: Vec::new(),
            account: None,
            terms: BTreeMap::new(),
            stops: BTreeMap::new(),
            triggered: std::collections::BTreeSet::new(),
        }
    }

    /// An engine whose fills move `account` (`docs/decisions/0003-sandbox.md`).
    #[must_use]
    pub fn with_account(model: SimulationModel, account: Account) -> Self {
        Self { account: Some(account), ..Self::new(model) }
    }

    #[must_use]
    pub fn account(&self) -> Option<&Account> {
        self.account.as_ref()
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn faults(&self) -> &[String] {
        &self.faults
    }

    pub fn model(&self) -> SimulationModel {
        self.model
    }

    fn record(&mut self, client_id: &str, event: &OrderEvent, at_ns: i128) {
        if let Err(e) = self.ledger.apply(client_id, event, at_ns) {
            self.faults.push(e.to_string());
        }
    }

    /// Validated the way a venue validates, before anything is queued. A refusal leaves no order.
    pub fn submit(
        &mut self,
        declaration: &CapabilityDeclaration,
        order: &OrderRequest,
        now_ns: i128,
    ) -> VenueOutcome {
        if self.ledger.order(&order.client_id).is_some() {
            return VenueOutcome::Rejected(Rejection::Duplicate);
        }
        if let Some(refusal) = refuse(declaration, order) {
            return VenueOutcome::Rejected(refusal);
        }
        // A session's day, open and close need a calendar this venue does not keep.
        if matches!(
            order.terms.time_in_force,
            TimeInForce::Day | TimeInForce::AtTheOpen | TimeInForce::AtTheClose
        ) {
            return bijective_venue::normalise::terms_not_taken(order.terms);
        }
        if let Err(refusal) = self.afford(declaration, order) {
            return VenueOutcome::Rejected(refusal);
        }
        self.next_order += 1;
        let venue_order_id = format!("sim-{}", self.next_order);
        if let Err(e) = self.ledger.create(order.clone()) {
            self.faults.push(e.to_string());
            return VenueOutcome::Rejected(Rejection::Duplicate);
        }
        self.record(&order.client_id, &OrderEvent::Submitted, now_ns);
        self.record(
            &order.client_id,
            &OrderEvent::Accepted { venue_order_id: Some(venue_order_id) },
            now_ns,
        );
        self.queue.push_back(Pending {
            client_id: order.client_id.clone(),
            command: Command::Place,
            effective_at_ns: now_ns + self.model.latency.submit_ns,
        });
        VenueOutcome::Accepted
    }

    /// A buy the account cannot pay for is refused as a venue refuses one, before anything is
    /// queued. A market buy is priced at the best ask it would take. With no ask it is refused.
    fn afford(
        &mut self,
        declaration: &CapabilityDeclaration,
        order: &OrderRequest,
    ) -> Result<(), Rejection> {
        let Ok(instrument) = declaration.instruments().get(&order.instrument) else {
            return Ok(());
        };
        let terms = Terms {
            currency: instrument.quote_currency.clone(),
            price_scale: instrument.precision.price_scale,
            quantity_scale: instrument.precision.quantity_scale,
        };
        let best_ask = || {
            self.books
                .get(&order.instrument)
                .and_then(|b| b.available(BookSide::Ask).first().map(|(p, _)| *p))
        };
        let price = order.price_scaled.or_else(best_ask);
        if let Some(account) = self.account.as_mut() {
            let Some(price) = price else {
                return Err(Rejection::Unclassified { venue_code: "simulated:no-ask".into() });
            };
            if !account.reserve(
                &order.client_id,
                &terms,
                order.side,
                (price, order.quantity_scaled),
            ) {
                return Err(Rejection::InsufficientMargin);
            }
        }
        self.terms.insert(order.client_id.clone(), terms);
        Ok(())
    }

    pub fn cancel(&mut self, client_id: &str, now_ns: i128) -> VenueOutcome {
        match self.ledger.order(client_id).map(|o| o.state) {
            Some(state) if state.may_execute() => {
                self.record(client_id, &OrderEvent::CancelRequested, now_ns);
                self.queue.push_back(Pending {
                    client_id: client_id.to_string(),
                    command: Command::Cancel,
                    effective_at_ns: now_ns + self.model.latency.cancel_ns,
                });
                VenueOutcome::Accepted
            }
            _ => VenueOutcome::Rejected(Rejection::Unclassified {
                venue_code: "simulated:no-open-order".into(),
            }),
        }
    }

    /// Phase one: the book moves, and orders already resting meet it.
    pub fn on_record(&mut self, record: &MarketRecord) {
        let book = self.books.entry(record.instrument.clone()).or_default();
        book.apply(&record.fact);
        self.match_resting(&record.instrument, record.exchange_time, record.received_at_ns);
        self.trigger_stops(&record.instrument, record.received_at_ns);
    }

    pub fn on_top(&mut self, top: &BookTop, received_at_ns: i128) {
        self.books.entry(top.instrument.clone()).or_default().apply_top(top);
        let at = ExchangeTime::from_ms(top.venue_ts_ms);
        self.match_resting(&top.instrument, at, received_at_ns);
        self.trigger_stops(&top.instrument, received_at_ns);
    }

    /// A buy stop triggers when the best ask reaches it, a sell stop when the best bid does: the
    /// top of book stands in for the last traded price a venue triggers on. A triggered stop is
    /// placed as the limit or market order it carries, against the book that triggered it.
    fn trigger_stops(&mut self, instrument: &VenueSymbol, now_ns: i128) {
        let held = self.stops.get(instrument).cloned().unwrap_or_default();
        for client_id in held {
            let Some(order) = self.ledger.order(&client_id).cloned() else { continue };
            let Some(stop) = order.request.terms.stop_price_scaled else { continue };
            let Some(book) = self.books.get(instrument) else { return };
            let reached = match order.request.side {
                Side::Buy => book.available(BookSide::Ask).first().is_some_and(|(p, _)| *p >= stop),
                Side::Sell => {
                    book.available(BookSide::Bid).first().is_some_and(|(p, _)| *p <= stop)
                }
            };
            if !reached {
                continue;
            }
            if let Some(queue) = self.stops.get_mut(instrument) {
                queue.retain(|id| id != &client_id);
            }
            self.triggered.insert(client_id.clone());
            self.place(&client_id, now_ns);
        }
    }

    /// Phase two: every command due by `now_ns` is applied in the order it was sent.
    pub fn settle(&mut self, now_ns: i128) {
        while self.queue.front().is_some_and(|p| p.effective_at_ns <= now_ns) {
            let Some(pending) = self.queue.pop_front() else { break };
            match pending.command {
                Command::Place => self.place(&pending.client_id, now_ns),
                Command::Cancel => self.withdraw(&pending.client_id, now_ns),
            }
        }
    }

    fn place(&mut self, client_id: &str, now_ns: i128) {
        let Some(order) = self.ledger.order(client_id).cloned() else { return };
        if order.state.is_terminal() {
            return;
        }
        let instrument = order.request.instrument.clone();
        let terms = order.request.terms;
        if terms.stop_price_scaled.is_some() && !self.triggered.contains(client_id) {
            self.stops.entry(instrument).or_default().push(client_id.to_string());
            return;
        }
        let crossing = self.crossing_quantity(&order.request);
        // Cancelled untouched: a post-only order that would take, a fill-or-kill the book cannot fill.
        let untouched = (terms.post_only && crossing > 0)
            || (terms.time_in_force == TimeInForce::FillOrKill
                && crossing < order.remaining_scaled());
        if untouched {
            self.record(client_id, &OrderEvent::Canceled, now_ns);
            self.release(client_id);
            return;
        }
        self.take_liquidity(&order.request, now_ns);
        let still_working = self.ledger.order(client_id).is_some_and(|o| o.state.may_execute());
        let rests = order.request.price_scaled.is_some() && terms.time_in_force.may_rest();
        match (still_working, rests) {
            (true, true) => self.resting.entry(instrument).or_default().push(client_id.to_string()),
            // A market or immediate order's remainder is not left resting. The venue cancels it.
            (true, false) => {
                self.record(client_id, &OrderEvent::Canceled, now_ns);
                self.release(client_id);
            }
            (false, _) => self.release(client_id),
        }
    }

    fn release(&mut self, client_id: &str) {
        if let Some(account) = self.account.as_mut() {
            account.release(client_id);
        }
    }

    fn withdraw(&mut self, client_id: &str, now_ns: i128) {
        if self.ledger.order(client_id).is_some_and(|o| o.state.may_execute()) {
            self.record(client_id, &OrderEvent::Canceled, now_ns);
        }
        self.release(client_id);
        for queue in self.resting.values_mut().chain(self.stops.values_mut()) {
            queue.retain(|id| id != client_id);
        }
    }

    /// How much of the opposite side lies within the order's limit, as an arriving order meets it.
    fn crossing_quantity(&self, request: &OrderRequest) -> i128 {
        let Some(book) = self.books.get(&request.instrument) else { return 0 };
        book.available(opposite(request.side))
            .into_iter()
            .take_while(|(price, _)| within(request.side, *price, request.price_scaled))
            .map(|(_, quantity)| quantity)
            .sum()
    }

    /// An arriving order takes the opposite side best first, at each level's price, up to its limit.
    fn take_liquidity(&mut self, request: &OrderRequest, now_ns: i128) {
        let opposite = opposite(request.side);
        let Some(book) = self.books.get(&request.instrument) else { return };
        let levels = book.available(opposite);
        let mut remaining = self.remaining(&request.client_id);
        for (price, quantity) in levels {
            if remaining == 0 || !within(request.side, price, request.price_scaled) {
                break;
            }
            let taken = remaining.min(quantity);
            self.fill(request, price, taken, Liquidity::Taker, ExchangeTime::NotPublished, now_ns);
            if let Some(book) = self.books.get_mut(&request.instrument) {
                book.take(opposite, price, taken);
            }
            remaining -= taken;
        }
    }

    /// A resting order fills at its own price when the opposite side reaches it under the model.
    fn match_resting(&mut self, instrument: &VenueSymbol, at: ExchangeTime, now_ns: i128) {
        let waiting = self.resting.get(instrument).cloned().unwrap_or_default();
        for client_id in waiting {
            let Some(order) = self.ledger.order(&client_id).cloned() else { continue };
            let Some(limit) = order.request.price_scaled else { continue };
            let opposite = opposite(order.request.side);
            let Some(book) = self.books.get(instrument) else { return };
            let crossing: Vec<(i128, i128)> = book
                .available(opposite)
                .into_iter()
                .take_while(|(price, _)| {
                    crosses(self.model.fill, order.request.side, *price, limit)
                })
                .collect();
            let mut remaining = order.remaining_scaled();
            for (price, quantity) in crossing {
                if remaining == 0 {
                    break;
                }
                let taken = remaining.min(quantity);
                self.fill(&order.request, limit, taken, Liquidity::Maker, at, now_ns);
                if let Some(book) = self.books.get_mut(instrument) {
                    book.take(opposite, price, taken);
                }
                remaining -= taken;
            }
            if remaining == 0 {
                if let Some(queue) = self.resting.get_mut(instrument) {
                    queue.retain(|id| id != &client_id);
                }
                self.release(&client_id);
            }
        }
    }

    fn remaining(&self, client_id: &str) -> i128 {
        self.ledger.order(client_id).map_or(0, |o| o.remaining_scaled())
    }

    fn fill(
        &mut self,
        request: &OrderRequest,
        price: i128,
        quantity: i128,
        liquidity: Liquidity,
        at: ExchangeTime,
        now_ns: i128,
    ) {
        self.next_fill += 1;
        let venue_order_id =
            self.ledger.order(&request.client_id).and_then(|o| o.venue_order_id.clone());
        let fee = match (self.account.as_mut(), self.terms.get(&request.client_id)) {
            (Some(account), Some(terms)) => {
                account.fill(&request.client_id, terms, request.side, (price, quantity, liquidity))
            }
            _ => None,
        };
        let fill = bijective_lifecycle::Fill {
            fill_id: format!("sim-fill-{}", self.next_fill),
            venue_order_id,
            side: request.side,
            price_scaled: price,
            quantity_scaled: quantity,
            fee,
            liquidity,
            exchange_time: at,
            received_at_ns: now_ns,
        };
        self.record(&request.client_id, &OrderEvent::Filled(fill.clone()), now_ns);
        self.traded.insert(request.instrument.clone(), ());
        self.fills.push(FillReport {
            client_id: Some(request.client_id.clone()),
            instrument: request.instrument.clone(),
            fill,
        });
    }

    /// Everything the venue holds, as a venue reports it. Complete by construction: the engine is
    /// the whole venue. A traded instrument now flat is reported as an explicit flat.
    pub fn evidence(&self) -> VenueEvidence {
        let orders = self
            .ledger
            .orders()
            .map(|o| OrderStatusReport {
                client_id: Some(o.client_id().to_string()),
                venue_order_id: o.venue_order_id.clone().unwrap_or_default(),
                instrument: o.request.instrument.clone(),
                side: o.request.side,
                ordered_quantity_scaled: o.request.quantity_scaled,
                filled_quantity_scaled: o.filled_quantity_scaled,
                status: reported(o.state),
                reported_at: ExchangeTime::NotPublished,
            })
            .collect();
        let positions = self
            .traded
            .keys()
            .map(|i| PositionReport {
                instrument: i.clone(),
                quantity_scaled: self.ledger.position(i).map_or(0, |p| p.quantity_scaled()),
            })
            .collect();
        VenueEvidence {
            charges: Vec::new(),
            orders,
            fills: self.fills.clone(),
            positions: PositionEvidence::Reported(positions),
            history_from_ns: None,
            complete: true,
        }
    }

    pub fn fills_since(&self, from: usize) -> &[FillReport] {
        self.fills.get(from..).unwrap_or(&[])
    }
}

fn opposite(side: Side) -> BookSide {
    match side {
        Side::Buy => BookSide::Ask,
        Side::Sell => BookSide::Bid,
    }
}

/// Whether an arriving order may take a level: at or better than its limit, a market order any.
fn within(side: Side, price: i128, limit: Option<i128>) -> bool {
    match (side, limit) {
        (_, None) => true,
        (Side::Buy, Some(l)) => price <= l,
        (Side::Sell, Some(l)) => price >= l,
    }
}

fn crosses(model: FillModel, side: Side, opposite_price: i128, limit: i128) -> bool {
    match (model, side) {
        (FillModel::TradeThrough, Side::Buy) => opposite_price < limit,
        (FillModel::TradeThrough, Side::Sell) => opposite_price > limit,
        (FillModel::Touch, Side::Buy) => opposite_price <= limit,
        (FillModel::Touch, Side::Sell) => opposite_price >= limit,
    }
}

fn reported(state: OrderState) -> ReportedStatus {
    match state {
        OrderState::Filled => ReportedStatus::Filled,
        OrderState::Canceled => ReportedStatus::Canceled,
        OrderState::Expired => ReportedStatus::Expired,
        OrderState::Rejected | OrderState::NotSent => ReportedStatus::Rejected,
        _ => ReportedStatus::Working,
    }
}

/// The venue's own validation: a known instrument that is trading, a quantity at or above the
/// minimum and on the lot, a price on the tick. A refusal is a definitive venue rejection: nothing
/// was placed. Codes are the simulation's own, carried rather than mapped to a live venue's.
fn refuse(declaration: &CapabilityDeclaration, order: &OrderRequest) -> Option<Rejection> {
    let Ok(instrument) = declaration.instruments().get(&order.instrument) else {
        return Some(Rejection::UnknownInstrument);
    };
    let code = |c: &str| Some(Rejection::Unclassified { venue_code: format!("simulated:{c}") });
    if instrument.status != InstrumentStatus::Trading {
        return code("not-trading");
    }
    if order.quantity_scaled < instrument.minimum_size.max(1) {
        return Some(Rejection::BelowMinimumSize);
    }
    if instrument.precision.check_quantity(order.quantity_scaled).is_err() {
        return code("quantity-off-lot");
    }
    match order.price_scaled {
        Some(p) if p <= 0 || instrument.precision.check_price(p).is_err() => code("price-off-tick"),
        _ => None,
    }
}
