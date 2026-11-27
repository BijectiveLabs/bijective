// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The backtest loop (`docs/decisions/0015-backtest-through-the-order-path.md`): the recording
//! moves the clock and places the passes, the simulated venues fill, and the result is printed
//! when the window ends.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bijective_common::readiness::Surface;
use bijective_control_engine::backtest::{Decided, MarketEntry, Step, schedule};
use bijective_control_engine::{ControlEvent, RefusalReason, RunMode, RunPlan};
use bijective_event_store::store::{end_of, read_range};
use bijective_serialization::signal::node::BACKTEST;

use crate::market::{Bound, received_at_ns};
use crate::node::Node;
use crate::paper::Money;
use crate::{Fatal, HEARTBEAT};

/// Where a backtest reads its market.
pub(crate) struct Window {
    pub(crate) market_store: PathBuf,
    pub(crate) market: String,
}

impl Window {
    /// Required for a backtest and refused for any other mode, so a flag never changes nothing.
    pub(crate) fn from_args(
        plan: RunPlan,
        (market_store, market): (Option<PathBuf>, Option<String>),
        drill_seconds: Option<u64>,
    ) -> Result<Option<Self>, Fatal> {
        if plan.mode() != RunMode::Backtest {
            if market_store.is_some() || market.is_some() {
                return Err(Fatal::usage(
                    "--market-store and --market belong to --mode backtest; a paper or live node \
                     reads the market in its own store",
                ));
            }
            return Ok(None);
        }
        let (Some(market_store), Some(market)) = (market_store, market) else {
            return Err(Fatal::usage(
                "--mode backtest needs --market-store <recorded store> and --market <stream>",
            ));
        };
        if drill_seconds.is_some() {
            return Err(Fatal::usage(
                "--drill-seconds bounds a paper or live run; a backtest ends with its recording",
            ));
        }
        Ok(Some(Self { market_store, market }))
    }
}

/// Runs every pass the recording calls for, one closing pass, and prints the result.
pub(crate) async fn run(mut node: Node, surface: &Surface, window: &Window) -> Result<(), Fatal> {
    let market = recorded_market(window)?;
    let decided = decided_positions(node.store_root())?;
    let plan = schedule(&market, &decided, HEARTBEAT.as_nanos() as i128);
    let Some(last) = plan.steps.last().copied() else {
        return Err(Fatal::usage(format!("{} holds nothing to run against", window.market)));
    };
    let mut result = Outcome::start(node.simulated().money(node.marks()));
    for step in plan.steps.iter().chain(std::iter::once(&last)) {
        take(&mut node, surface, window, step).await?;
        result.observe(node.simulated().money(node.marks()));
    }
    let first = plan.steps[0];
    println!(
        "backtest: {} pass(es) over positions {} to {} of {}",
        plan.steps.len() + 1,
        first.position,
        last.position,
        window.market
    );
    println!("proposals: {} read, {} decided past the window's end", decided.len(), plan.unreached);
    report_simulated(&node, BACKTEST.control);
    result.report();
    println!("model:    {}", crate::paper::MODEL.describe());
    println!("not measured: the strategy was told nothing of its orders (decision 0015, stage 1)");
    println!("record:   {} ({})", node.store_root().display(), BACKTEST.control);
    Ok(())
}

async fn take(
    node: &mut Node,
    surface: &Surface,
    window: &Window,
    step: &Step,
) -> Result<(), Fatal> {
    crate::clock::advance_to(step.at_ns).map_err(Fatal::failure)?;
    node.bound_to(Bound {
        market: window.market.clone(),
        through: step.position,
        through_ns: step.at_ns,
        proposals_through: step.proposals_through,
    });
    node.pass(surface).await
}

/// Every entry of the driving stream, with the instant the simulated venue is fed it at.
fn recorded_market(window: &Window) -> Result<Vec<MarketEntry>, Fatal> {
    let entries = read_range(&window.market_store, &window.market, 0, u64::MAX)
        .map_err(|e| Fatal::usage(format!("{} could not be read: {e:?}", window.market)))?;
    let mut market = Vec::with_capacity(entries.len());
    for entry in entries {
        let update =
            bijective_serialization::signal::market::decode(&entry.payload).map_err(|e| {
                Fatal::usage(format!(
                    "{} position {} is unreadable: {e:?}",
                    window.market, entry.seq
                ))
            })?;
        market.push(MarketEntry { position: entry.seq, at_ns: received_at_ns(&update) });
    }
    Ok(market)
}

/// The market position each proposal was decided at. A proposal that states none cannot be
/// stepped, so the run is refused rather than judging it at a position it was not decided at.
fn decided_positions(record: &Path) -> Result<Vec<Decided>, Fatal> {
    let held = end_of(record, BACKTEST.proposals)
        .map_err(|e| Fatal::usage(format!("{} could not be read: {e:?}", BACKTEST.proposals)))?;
    if held.is_none() {
        return Ok(Vec::new());
    }
    let entries = read_range(record, BACKTEST.proposals, 0, u64::MAX)
        .map_err(|e| Fatal::usage(format!("{} could not be read: {e:?}", BACKTEST.proposals)))?;
    let mut decided = Vec::with_capacity(entries.len());
    let mut unplaced = 0usize;
    for entry in entries {
        // An unreadable proposal is the order path's to refuse and record, at the position it is on.
        let at_position = match bijective_serialization::signal::decode(&entry.payload) {
            Ok(proposal) => proposal.decided_at_position,
            Err(_) => Some(decided.last().map_or(0, |d: &Decided| d.at_position)),
        };
        match at_position {
            Some(at_position) => decided.push(Decided { seq: entry.seq, at_position }),
            None => unplaced += 1,
        }
    }
    if unplaced > 0 {
        return Err(Fatal::usage(format!(
            "{unplaced} proposal(s) state no market position, as a whole-exchange run writes them; \
             a backtest judges each where it was decided, so run the strategy stepwise"
        )));
    }
    Ok(decided)
}

/// A paper run's result when its drill ends: the lines a backtest prints, and money as it stands.
pub(crate) fn report_paper(node: &Node) {
    report_simulated(node, bijective_serialization::signal::node::PAPER.control);
    let scale = bijective_venue_simulated::MONEY_SCALE;
    for m in node.simulated().money(node.marks()) {
        let equity = m.equity_scaled.map_or_else(
            || "unmeasured: a position had no mark".to_string(),
            |e| decimal(e, scale),
        );
        println!(
            "{} {}: cash {}; equity {equity}",
            m.venue,
            m.currency,
            decimal(m.cash_scaled, scale)
        );
    }
}

/// What a run on simulated venues came to: orders, fills, holdings, and the refusals on `control`.
pub(crate) fn report_simulated(node: &Node, control: &str) {
    node.simulated().report(node.marks());
    report_refusals(node.store_root(), control);
}

/// How many proposals the order path refused, and why the first was, read back from the record.
fn report_refusals(record: &Path, control: &str) {
    let Ok(entries) = read_range(record, control, 0, u64::MAX) else {
        println!("refused:  unread, {control} could not be read");
        return;
    };
    let refused: Vec<(String, RefusalReason)> = entries
        .iter()
        .filter_map(|entry| match bijective_control_engine::wire::decode(&entry.payload) {
            Ok(ControlEvent::Refused { decision_id, reason, .. }) => Some((decision_id, reason)),
            _ => None,
        })
        .collect();
    match refused.first() {
        None => println!("refused:  none"),
        Some((id, reason)) => {
            println!("refused:  {} proposal(s); the first, {id:?}: {reason:?}", refused.len())
        }
    }
}

/// Cash and equity per venue and currency across the run, and the largest fall from a peak.
struct Outcome {
    start: BTreeMap<(String, String), Money>,
    end: BTreeMap<(String, String), Money>,
    peak: BTreeMap<(String, String), i128>,
    drawdown: BTreeMap<(String, String), i128>,
    unmarked: usize,
}

impl Outcome {
    fn start(money: Vec<Money>) -> Self {
        let start: BTreeMap<_, _> =
            money.into_iter().map(|m| ((m.venue.clone(), m.currency.clone()), m)).collect();
        let peak =
            start.iter().filter_map(|(key, m)| Some((key.clone(), m.equity_scaled?))).collect();
        Self { end: start.clone(), start, peak, drawdown: BTreeMap::new(), unmarked: 0 }
    }

    fn observe(&mut self, money: Vec<Money>) {
        for m in money {
            let key = (m.venue.clone(), m.currency.clone());
            match m.equity_scaled {
                Some(equity) => {
                    let peak = self.peak.entry(key.clone()).or_insert(equity);
                    *peak = (*peak).max(equity);
                    let fall = *peak - equity;
                    let worst = self.drawdown.entry(key.clone()).or_insert(0);
                    *worst = (*worst).max(fall);
                }
                None => self.unmarked += 1,
            }
            self.end.insert(key, m);
        }
    }

    fn report(&self) {
        let scale = bijective_venue_simulated::MONEY_SCALE;
        for ((venue, currency), end) in &self.end {
            let key = (venue.clone(), currency.clone());
            let start = self.start.get(&key);
            let cash_start = start.map_or(0, |m| m.cash_scaled);
            let change = match (start.and_then(|m| m.equity_scaled), end.equity_scaled) {
                (Some(was), Some(now)) => decimal(now - was, scale),
                _ => "unmeasured: a position had no mark".to_string(),
            };
            println!(
                "{venue} {currency}: cash {} -> {}; equity change {change}; largest drawdown {}",
                decimal(cash_start, scale),
                decimal(end.cash_scaled, scale),
                decimal(self.drawdown.get(&key).copied().unwrap_or(0), scale),
            );
        }
        if self.unmarked > 0 {
            println!("unmarked: {} pass reading(s) held a position with no mark", self.unmarked);
        }
    }
}

/// A scaled integer as a decimal, sign first.
pub(crate) fn decimal(value: i128, scale: u8) -> String {
    let divisor = 10u128.pow(u32::from(scale));
    let magnitude = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{magnitude}");
    }
    let width = usize::from(scale);
    format!("{sign}{}.{:0width$}", magnitude / divisor, magnitude % divisor)
}
