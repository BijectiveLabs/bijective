// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Paper runs and backtests on the simulated venue: an order is matched against the market this
//! node reads, so fills, positions, reconciliation and limits run the live code, with no order
//! leaving the node and no credential read. `docs/concepts/simulated_venue.md`, decision 0015.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use bijective_model::{
    AccountSnapshot, BookTop, Clearance, InstrumentId, OrderRequest, VenueSymbol,
};
use bijective_risk::valuation::Mark;
use bijective_serialization::signal::market::BookUpdate;
use bijective_venue::contract::{
    Answer, CapabilityDeclaration, ConnectError, VenueEvidence, VenueExecution,
};
use bijective_venue::normalise::VenueOutcome;
use bijective_venue::profile::Instrument;
use bijective_venue_simulated::{
    Account, FeeRates, FillModel, LatencyModel, MONEY_SCALE, SimulatedVenue, SimulationModel,
};

use crate::venues::{OpenVenues, Withdrawn};

/// What a paper run assumes about execution. A resting order fills only when the market trades
/// through it, because its place in the queue is unknown, and no latency is modelled. Fees and
/// balances are the sandbox account's, stated per venue at start.
pub(crate) const MODEL: SimulationModel = SimulationModel {
    fill: FillModel::TradeThrough,
    latency: LatencyModel { submit_ns: 0, cancel_ns: 0 },
};

/// The simulated venues of a paper run or a backtest, each fed the market stream of the venue it
/// stands for. Empty in a live run.
#[derive(Default)]
pub(crate) struct Paper {
    venues: Vec<Fed>,
}

struct Fed {
    venue: String,
    /// The real venue's, for the terms an order and a position are priced in.
    declaration: CapabilityDeclaration,
    /// The currencies its sandbox account started with.
    currencies: Vec<String>,
    stream: String,
    shared: Arc<Mutex<SimulatedVenue>>,
    /// Platform identity to the venue's symbol, from the venue's own catalogue.
    symbols: BTreeMap<InstrumentId, VenueSymbol>,
}

impl Paper {
    /// One sandbox venue per configured venue, from its profile, its declared starting balances
    /// and its declared fees (`docs/decisions/0003-sandbox.md`). A venue without balances, or
    /// whose profile cannot be read, is withdrawn, as an adapter that cannot open is in a live run.
    pub(crate) fn open(
        accepted: &bijective_control_engine::Accepted,
        store_root: &std::path::Path,
    ) -> (Self, OpenVenues, Vec<Withdrawn>) {
        let mut paper = Paper::default();
        let mut opened: OpenVenues = Vec::new();
        let mut withdrawn = Vec::new();
        for access in accepted.access() {
            let declaration = match declaration_of(&access.profile, accepted.assets()) {
                Ok(d) => d,
                Err(reason) => {
                    withdrawn.push(Withdrawn { venue: access.name.clone(), reason });
                    continue;
                }
            };
            let Some(balances) = accepted.sandbox(&access.name) else {
                let reason = "no sandbox balances are declared for it".to_string();
                withdrawn.push(Withdrawn { venue: access.name.clone(), reason });
                continue;
            };
            let fees = fees_of(store_root, access.dialect());
            eprintln!(
                "{}: sandbox {}: balances {balances:?}, fees {fees:?}",
                crate::PROGRAM,
                access.name
            );
            let account = Account::new(balances.clone(), fees);
            let symbols = declaration
                .instruments()
                .iter()
                .map(|i| (i.id.clone(), i.symbol.clone()))
                .collect();
            let shared = Arc::new(Mutex::new(SimulatedVenue::with_account(
                declaration.clone(),
                MODEL,
                account,
            )));
            paper.venues.push(Fed {
                venue: access.name.clone(),
                declaration: declaration.clone(),
                currencies: balances.keys().cloned().collect(),
                stream: bijective_serialization::signal::market::stream(access.dialect()),
                shared: Arc::clone(&shared),
                symbols,
            });
            opened.push((access.name.clone(), Box::new(Shared { declaration, shared })));
        }
        (paper, opened, withdrawn)
    }

    /// Each simulated venue takes back what the paper record says it held (decision 0018), so a
    /// restarted paper node continues the run instead of starting a new account under the old
    /// record. A venue the record never traded on starts as declared.
    pub(crate) fn restore(&self, orders: &bijective_control_engine::orders::Orders, now_ns: i128) {
        for fed in &self.venues {
            let Some((_, ledger)) = orders.ledgers().find(|(venue, _)| *venue == fed.venue) else {
                continue;
            };
            let mut venue = lock(&fed.shared);
            let restored = venue.engine_mut().restore(&fed.declaration, ledger, now_ns);
            eprintln!(
                "{}: paper {}: restored {} order(s), {} fill(s), {} working, from the record",
                crate::PROGRAM,
                fed.venue,
                restored.orders,
                restored.fills,
                restored.working
            );
            for fault in venue.engine().faults() {
                eprintln!("{}: paper {}: {fault}", crate::PROGRAM, fed.venue);
            }
        }
    }

    /// The book moves on the venue whose stream it came from, and resting orders meet it. An
    /// update for an instrument the venue does not list moves nothing.
    pub(crate) fn feed(&self, stream: &str, update: &BookUpdate) {
        for fed in self.venues.iter().filter(|f| f.stream == stream) {
            let Some(symbol) = fed.symbols.get(&update.instrument) else { continue };
            let top = BookTop {
                instrument: symbol.clone(),
                sequence: update.seq,
                venue_ts_ms: update.venue_ts_ms,
                bid_price: update.bid_price,
                bid_quantity: update.bid_quantity,
                ask_price: update.ask_price,
                ask_quantity: update.ask_quantity,
                bid_depth: update.bid_depth,
                ask_depth: update.ask_depth,
            };
            let received = crate::market::received_at_ns(update);
            lock(&fed.shared).engine_mut().on_top(&top, received);
        }
    }

    /// Orders sent since the last pass reach the matching engine and meet the current book.
    pub(crate) fn settle(&self, now_ns: i128) {
        for fed in &self.venues {
            lock(&fed.shared).engine_mut().settle(now_ns);
        }
    }
}

/// One venue's money in one currency: its cash, and its cash with every open position priced in
/// that currency at its mark, `None` where a position has no mark.
#[derive(Debug, Clone)]
pub(crate) struct Money {
    pub(crate) venue: String,
    pub(crate) currency: String,
    pub(crate) cash_scaled: i128,
    pub(crate) equity_scaled: Option<i128>,
}

impl Paper {
    /// Every venue's money per currency at `marks`, at the sandbox account's scale.
    pub(crate) fn money(&self, marks: &BTreeMap<InstrumentId, Mark>) -> Vec<Money> {
        let mut money = Vec::new();
        for fed in &self.venues {
            let venue = lock(&fed.shared);
            let engine = venue.engine();
            let cash = |currency: &str| engine.account().map_or(0, |a| a.cash(currency));
            let mut equity: BTreeMap<String, Option<i128>> =
                fed.currencies.iter().map(|c| (c.clone(), Some(cash(c)))).collect();
            for (symbol, quantity) in engine.ledger().open_positions() {
                let Ok(listed) = fed.declaration.instruments().get(symbol) else { continue };
                let held =
                    marks.get(&listed.id).and_then(|m| priced(m.price_scaled, quantity, listed));
                let currency = listed.quote_currency.clone();
                let slot = equity.entry(currency.clone()).or_insert_with(|| Some(cash(&currency)));
                *slot = slot.zip(held).map(|(sum, value)| sum + value);
            }
            for (currency, equity_scaled) in equity {
                let cash_scaled = cash(&currency);
                money.push(Money {
                    venue: fed.venue.clone(),
                    currency,
                    cash_scaled,
                    equity_scaled,
                });
            }
        }
        money
    }

    /// Whether this node trades on simulated venues: a paper run or a backtest.
    pub(crate) fn is_simulated(&self) -> bool {
        !self.venues.is_empty()
    }

    /// Every venue's equity per currency at `marks`, as a venue would state its account's.
    /// `None` when a position has no mark.
    pub(crate) fn stated_equity(
        &self,
        marks: &BTreeMap<InstrumentId, Mark>,
    ) -> Option<Vec<bijective_risk::valuation::margin::StatedEquity>> {
        self.money(marks)
            .into_iter()
            .map(|m| {
                Some(bijective_risk::valuation::margin::StatedEquity {
                    venue: m.venue,
                    currency: m.currency,
                    equity_scaled: m.equity_scaled?,
                    scale: MONEY_SCALE,
                })
            })
            .collect()
    }

    /// Per venue: the orders it took, its fills and what it holds at `marks`.
    pub(crate) fn report(&self, marks: &BTreeMap<InstrumentId, Mark>) {
        for fed in &self.venues {
            let venue = lock(&fed.shared);
            let engine = venue.engine();
            let orders = engine.ledger().orders().count();
            let fills = engine.fills_since(0).len();
            println!("{}: {orders} order(s) at the venue, {fills} fill(s)", fed.venue);
            for (symbol, quantity) in engine.ledger().open_positions() {
                let Ok(listed) = fed.declaration.instruments().get(symbol) else { continue };
                let scales = &listed.precision;
                let mark = marks.get(&listed.id).map_or_else(
                    || "none".to_string(),
                    |m| crate::backtest::decimal(m.price_scaled, scales.price_scale),
                );
                let held = crate::backtest::decimal(quantity, scales.quantity_scale);
                println!("{}: holds {held} {} at mark {mark}", fed.venue, listed.id);
            }
        }
    }
}

/// `quantity` at `price` as money at the sandbox account's scale, truncated toward zero.
fn priced(price: i128, quantity: i128, listed: &Instrument) -> Option<i128> {
    let raw = price.checked_mul(quantity)?;
    let from = u32::from(listed.precision.price_scale) + u32::from(listed.precision.quantity_scale);
    let to = u32::from(MONEY_SCALE);
    if from <= to {
        return raw.checked_mul(10i128.checked_pow(to - from)?);
    }
    Some(raw / 10i128.checked_pow(from - to)?)
}

/// The venue's declared fees, or none: an undeclared fee is not charged and the start says so.
fn fees_of(store_root: &std::path::Path, venue: &str) -> FeeRates {
    match bijective_serialization::signal::fees::standing(store_root, venue) {
        Ok(Some(fees)) => {
            FeeRates { taker_bps_scaled: Some(fees.taker), maker_bps_scaled: Some(fees.maker) }
        }
        _ => FeeRates::default(),
    }
}

fn declaration_of(
    profile: &str,
    assets: &bijective_control_engine::config::Assets,
) -> Result<CapabilityDeclaration, String> {
    let profile = crate::venues::read_profile(profile, assets)?;
    Ok(CapabilityDeclaration::from_profile(&profile))
}

/// A poisoned lock means a call panicked mid-way. The engine's own state machine still refuses
/// what it cannot apply, so the state is read rather than the run stopped.
fn lock(shared: &Mutex<SimulatedVenue>) -> std::sync::MutexGuard<'_, SimulatedVenue> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The order path's handle on one simulated venue. The node keeps the other to feed it.
struct Shared {
    declaration: CapabilityDeclaration,
    shared: Arc<Mutex<SimulatedVenue>>,
}

impl VenueExecution for Shared {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        order: &'a OrderRequest,
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        let outcome = lock(&self.shared).engine_mut().submit(&self.declaration, order, now_ns);
        Box::pin(async move { outcome })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        client_id: &'a str,
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        let outcome = lock(&self.shared).engine_mut().cancel(client_id, now_ns);
        Box::pin(async move { outcome })
    }

    fn account(&mut self, now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        let snapshot = lock(&self.shared).account_now(now_ns);
        Box::pin(async move { snapshot })
    }

    /// The whole simulated venue, complete by construction.
    fn evidence(
        &mut self,
        _since_ns: i128,
        _now_ns: i128,
    ) -> Answer<'_, Result<VenueEvidence, ConnectError>> {
        let evidence = lock(&self.shared).engine().evidence();
        Box::pin(async move { Ok(evidence) })
    }
}
