// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Node start-up and one decision pass: commands, market, positions, limits, proposals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bijective_common::readiness::{Health, Surface};
use bijective_control_engine::wire::Participation;
use bijective_control_engine::{
    Accepted, Judgement, OrderPath, ProposalWindow, Reachable, RunMode, RunPlan, Startup, assess,
    carry_out_commands, drive,
};
use bijective_limits::{Evaluation, Limit, PositionHeld, evaluate};
use bijective_model::InstrumentId;
use bijective_risk::valuation::currency::Rates;
use bijective_risk::valuation::{Pricing, Valuation, Venues, value};
use bijective_serialization::signal::Instruction;
use bijective_venue::contract::VenueExecution;
use bijective_venue::profile::Instrument;

use crate::attribution::{Scene, attach_attribution};
use crate::clock::{clock, now_ns};
use crate::daily_loss::{LossInputs, attach_daily_loss};
use crate::market::{Bound, MarketView, read_positions};
use crate::paper::{MODEL, Paper};
use crate::startup::{
    catalogue_by_identity, declare_limits, put_limits_in_force, rates_of, record_withdrawn_at_open,
    unverified_venues,
};
use crate::venues::{OpenVenues, Withdrawn, open_for};
use crate::{Fatal, PROGRAM, streams};

/// What `start` needs beyond the accepted configuration.
pub(crate) struct Launch {
    pub(crate) store_root: PathBuf,
    /// Where the recorded market and fees are read: the store itself, except in a backtest.
    pub(crate) inputs_root: PathBuf,
    pub(crate) plan: RunPlan,
    pub(crate) keystore: Option<PathBuf>,
    pub(crate) credential: Option<String>,
    pub(crate) operators: Option<bijective_identity::OperatorRegistry>,
}

/// A started node: the order path, the open venues and the state carried between passes.
pub(crate) struct Node {
    store_root: PathBuf,
    inputs_root: PathBuf,
    /// How far a backtest pass reads (decision 0015). `None` in every other run.
    bound: Option<Bound>,
    plan: RunPlan,
    accepted: Accepted,
    path: OrderPath,
    adapters: OpenVenues,
    withdrawn_at_open: Vec<Withdrawn>,
    unverified: Vec<String>,
    catalogue: BTreeMap<InstrumentId, Instrument>,
    /// The reporting currency and its rate markets, checked against `catalogue` at start.
    rates: Rates,
    declared: Vec<Limit>,
    /// Venues last recorded as withdrawn. Participation is written on change only.
    recorded_withdrawn: BTreeSet<String>,
    /// Venues an operator stopped routing to. Re-admission is a command, not a restart.
    by_operator: BTreeSet<String>,
    /// Adapters that panicked. Never called again, their state is unknown.
    faulted: BTreeSet<String>,
    market: MarketView,
    /// The simulated venues of a paper run, fed the market as it is read. Empty otherwise.
    paper: Paper,
    /// What the latest reconciliation concluded, for the readiness detail.
    reconciled: String,
    reconciled_at_ns: i128,
    checked_at_ns: i128,
    /// The count of unvalued fees last said, so the warning comes once per change, not per pass.
    fees_unvalued_said: u64,
    /// What each venue's push channel said at the last pass.
    channels: BTreeMap<String, bijective_venue::contract::Pushed>,
}

/// One evidence read per unsettled venue per minute. Settling an unknown order is not latency
/// critical: trading stays halted until it settles, and a tighter cadence only adds venue load.
const RECONCILE_INTERVAL_NS: i128 = 60_000_000_000;

/// Every venue's positions are re-read every five minutes while nothing is unsettled: a change
/// made outside the platform is found within that bound, and the read is one paged request set
/// per venue, small beside the order flow.
const POSITION_CHECK_INTERVAL_NS: i128 = 300_000_000_000;

impl Node {
    /// Replays limit state, opens venues and declares limits. The trading state stays halted.
    pub(crate) fn start(
        launch: Launch,
        accepted: Accepted,
        surface: &Surface,
    ) -> Result<Self, Fatal> {
        let path = open_path(&launch.store_root, launch.plan, surface)?;

        // After the limit replay. Recorded inputs open simulated venues only (decision 0015).
        let (adapters, withdrawn_at_open, paper) = open_for(
            launch.plan,
            &launch.inputs_root,
            &accepted,
            launch.keystore.as_deref(),
            launch.credential.as_deref(),
        )
        .map_err(Fatal::failure)?;
        for w in &withdrawn_at_open {
            eprintln!("{PROGRAM}: {} is withdrawn: {}", w.venue, w.reason);
        }
        announce_simulated(launch.plan);
        restore_paper(launch.plan, &path, &paper);
        // Before the first evidence read: a venue that knows orders only by hash would read this
        // platform's own orders as external.
        let mut adapters = adapters;
        for (venue, adapter) in &mut adapters {
            adapter.adopt(&path.recorded_for(venue));
        }
        let unverified = unverified_venues(&adapters);
        let catalogue = catalogue_by_identity(&adapters)?;
        let rates = rates_of(&accepted, &catalogue)?;

        let mut path = path;
        if let Some(operators) = launch.operators {
            path.set_operators(operators);
        }
        for (venue, rate) in accepted.order_rates() {
            path.set_order_rate(venue, *rate);
        }
        put_limits_in_force(&mut path, &accepted)
            .map_err(|e| Fatal::failure(format!("limits refused: {e}")))?;
        let declared = declare_limits(&accepted)?;
        record_withdrawn_at_open(&mut path, &withdrawn_at_open);

        // Sized from every venue carried, not those reachable now: a refused admission marks the
        // buffer degraded.
        let carried: usize =
            adapters.iter().map(|(_, a)| a.declaration().instruments().len()).sum();
        let carried = carried + accepted.reporting().map_or(0, |r| r.rates.len());
        // A proposal is judged up to a pass after it was written, so the decision books reach back
        // two heartbeats beyond the freshness bound.
        let heartbeats = 2 * crate::HEARTBEAT.as_nanos() as i128;
        let reach = accepted.freshness_bound_ns().saturating_add(heartbeats);
        let market = MarketView::new(carried.max(1), reach);
        let recorded_withdrawn = withdrawn_at_open.iter().map(|w| w.venue.clone()).collect();

        Ok(Self {
            store_root: launch.store_root,
            inputs_root: launch.inputs_root,
            bound: None,
            plan: launch.plan,
            accepted,
            path,
            adapters,
            withdrawn_at_open,
            unverified,
            catalogue,
            rates,
            declared,
            recorded_withdrawn,
            by_operator: BTreeSet::new(),
            faulted: BTreeSet::new(),
            market,
            paper,
            reconciled: "not reconciled yet".into(),
            reconciled_at_ns: 0,
            checked_at_ns: 0,
            fees_unvalued_said: 0,
            channels: BTreeMap::new(),
        })
    }

    /// Reads every live venue's evidence against the orders routed to it and records the outcome.
    pub(crate) async fn reconcile_at_start(&mut self) -> Result<(), Fatal> {
        self.reconcile_all("reconciled at start").await
    }

    /// The same read when a paper run stops, so what its simulated venues did after the last
    /// pass is in the record a restart rebuilds them from (decision 0018).
    pub(crate) async fn reconcile_at_close(&mut self) -> Result<(), Fatal> {
        self.reconcile_all("reconciled at close").await
    }

    async fn reconcile_all(&mut self, label: &str) -> Result<(), Fatal> {
        self.reconciled = crate::reconciliation::reconcile_every(
            &mut self.path,
            &mut self.adapters,
            &mut self.faulted,
            label,
        )
        .await?;
        self.reconciled_at_ns = now_ns();
        self.checked_at_ns = self.reconciled_at_ns;
        eprintln!("{PROGRAM}: {}", self.reconciled);
        Ok(())
    }

    /// Reconciles every venue on the position-check cadence, and in between, at most once per
    /// interval, the venues where something is unsettled, so an adjustment or a late venue record
    /// can lift the resume barrier without a restart and a change made outside the platform halts
    /// it within one check. A venue whose push channel reported a change is read on this pass.
    async fn reconcile_when_due(&mut self) -> Result<(), Fatal> {
        let now = now_ns();
        let interval = if self.plan.reaches_venues() { RECONCILE_INTERVAL_NS } else { 0 };
        let mut reported = crate::reconciliation::pushed_changes(
            &mut self.adapters,
            &self.faulted,
            &mut self.channels,
        );
        // A venue that showed an order filled before its fills is read again now, not an interval on.
        reported.extend(self.path.awaiting_fills());
        reported.sort_unstable();
        reported.dedup();
        let again = if now - self.checked_at_ns >= POSITION_CHECK_INTERVAL_NS {
            self.checked_at_ns = now;
            let every = crate::reconciliation::reconcile_every(
                &mut self.path,
                &mut self.adapters,
                &mut self.faulted,
                "position check",
            );
            Some(every.await?)
        } else if now - self.reconciled_at_ns >= interval {
            crate::reconciliation::reconcile_unsettled(
                &mut self.path,
                &mut self.adapters,
                &mut self.faulted,
                &reported,
            )
            .await?
        } else {
            // Read now, without moving the interval: the unsettled venues keep their own cadence.
            let read = crate::reconciliation::reconcile_reported(
                &mut self.path,
                &mut self.adapters,
                &mut self.faulted,
                &reported,
            );
            if let Some(clauses) = read.await? {
                eprintln!("{PROGRAM}: {clauses}");
                self.reconciled = clauses;
            }
            return Ok(());
        };
        self.reconciled_at_ns = now;
        if let Some(clauses) = again {
            eprintln!("{PROGRAM}: {clauses}");
            self.reconciled = clauses;
        }
        Ok(())
    }

    pub(crate) fn store_root(&self) -> &std::path::Path {
        &self.store_root
    }

    /// The position of the next command to carry out.
    pub(crate) fn commands_from(&self) -> u64 {
        self.path.commands_from()
    }

    pub(crate) fn marks(&self) -> &BTreeMap<InstrumentId, bijective_risk::valuation::Mark> {
        &self.market.marks
    }

    /// The simulated venues of a paper run or a backtest. Empty in a live run.
    pub(crate) fn simulated(&self) -> &Paper {
        &self.paper
    }

    /// Bounds what every later pass reads (decision 0015).
    pub(crate) fn bound_to(&mut self, bound: Bound) {
        self.bound = Some(bound);
    }

    /// One decision pass. Commands are applied first, so a halt takes effect before waiting
    /// proposals are judged.
    pub(crate) async fn pass(&mut self, surface: &Surface) -> Result<(), Fatal> {
        self.apply_commands()?;
        // After commands, so an adjustment carried out this pass is in the ledger it reads.
        self.reconcile_when_due().await?;

        self.read_markets();

        let holdings = read_positions(&mut self.adapters, &mut self.faulted).await;
        let mut withdrawn = holdings.withdrawn;
        // A venue that never opened is as unknown as one that stopped answering.
        withdrawn.extend(self.withdrawn_at_open.iter().map(|w| w.venue.clone()));
        withdrawn.extend(self.by_operator.iter().cloned());
        self.record_transitions(&withdrawn)?;

        let pricing = Pricing {
            marks: &self.market.marks,
            catalogue: &self.catalogue,
            rates: &self.rates,
            as_of_ns: now_ns(),
        };
        let read: Vec<String> = self
            .adapters
            .iter()
            .map(|(venue, _)| venue.clone())
            .filter(|venue| !withdrawn.contains(venue))
            .collect();
        let venues = Venues { read: &read, withdrawn: &withdrawn };
        let mut valued = value(&holdings.held, &pricing, &venues, holdings.observed_at_ns);
        let scene = Scene {
            path: &self.path,
            limits: self.accepted.limits(),
            mandates: self.accepted.mandates(),
            adapters: &self.adapters,
            held: &holdings.held,
            pricing,
        };
        attach_attribution(&scene, &mut valued);
        let inputs =
            LossInputs { limits: self.accepted.limits(), adapters: &self.adapters, pricing };
        let unvalued = attach_daily_loss(&mut self.path, &inputs, &mut valued, now_ns())?;
        if unvalued != self.fees_unvalued_said && unvalued > 0 {
            eprintln!(
                "{PROGRAM}: the daily loss leaves out {unvalued} fee(s) it could not value; the \
                 loss is larger than the figure shown by those fees"
            );
        }
        self.fees_unvalued_said = unvalued;
        let limits = self.accepted.limits();
        crate::margin::attach_margin(limits, &holdings.margins, pricing, &mut valued);
        let evaluation = evaluate(
            &self.declared,
            &valued.snapshot,
            now_ns(),
            self.accepted.freshness_bound_ns(),
        );
        let stated =
            crate::margin::stated_equity(&holdings.margins, &withdrawn, &self.paper, pricing);
        let equity = stated.and_then(|s| crate::margin::book_equity_of(&s, pricing, &valued));
        let figures = bijective_control_engine::summary::portfolio(&valued.snapshot, equity);
        self.path
            .record_valuation(figures, now_ns())
            .map_err(|e| Fatal::failure(format!("the portfolio summary was not recorded: {e}")))?;
        let scoped = bijective_control_engine::summary::scoped(&valued.snapshot);
        self.path
            .record_scopes(scoped, now_ns())
            .map_err(|e| Fatal::failure(format!("the scoped figures were not recorded: {e}")))?;
        // Answered before the drive: a new safe state on the same figure would replace the
        // before-figure and leave the first halt unverified.
        let answered = self
            .path
            .answer_postconditions(&valued.snapshot, now_ns())
            .map_err(|e| Fatal::failure(format!("safe-state postcondition unrecorded: {e}")))?;
        let standing = self.standing(&valued, &evaluation, answered);
        self.answer_proposals(&evaluation, &withdrawn, &standing, surface).await
    }

    /// Reads every venue's market and facts stream, and the rate markets, then lets the
    /// simulated venues of a paper run settle against what was read.
    fn read_markets(&mut self) {
        let rate_markets = self.accepted.reporting().map(|r| r.markets.as_slice()).unwrap_or(&[]);
        let streams: Vec<String> = self
            .accepted
            .access()
            .iter()
            .map(bijective_control_engine::config::VenueAccess::dialect)
            .chain(rate_markets.iter().map(|m| m.name.as_str()))
            .map(bijective_serialization::signal::market::stream)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.market.absorb(&self.inputs_root, &streams, now_ns(), &self.paper, self.bound.as_ref());
        let facts: Vec<String> = self
            .accepted
            .access()
            .iter()
            .map(|a| bijective_serialization::signal::facts::stream(a.dialect()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let (catalogue, bound) = (&self.catalogue, self.bound.as_ref());
        self.market.absorb_facts(&self.inputs_root, &facts, now_ns(), catalogue, bound);
        self.paper.settle(now_ns());
    }

    /// Carries out operator commands. Venue routing commands are applied here, where the
    /// adapters live.
    fn apply_commands(&mut self) -> Result<(), Fatal> {
        let commands = streams(self.plan).commands;
        let carried = carry_out_commands(&mut self.path, clock(), &self.store_root, commands)
            .map_err(|e| Fatal::failure(format!("command path refused: {e}")))?;
        for c in carried.iter().filter(|c| c.carried_out) {
            match &c.instruction {
                Instruction::WithdrawVenue { venue } => {
                    self.by_operator.insert(venue.clone());
                }
                Instruction::AdmitVenue { venue } => {
                    self.by_operator.remove(venue);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Records participation changes only, so an hour-long outage is two records, not thousands.
    /// A write that fails stops the program: a withdrawal the stream does not hold would leave the
    /// console and the next start reading a venue as routed when this node had stopped routing to it.
    fn record_transitions(&mut self, withdrawn: &[String]) -> Result<(), Fatal> {
        let now: BTreeSet<String> = withdrawn.iter().cloned().collect();
        for venue in now.difference(&self.recorded_withdrawn) {
            let reason = "its account could not be read".to_string();
            self.path
                .record_participation(venue, Participation::Withdrawn { reason }, now_ns())
                .map_err(|e| Fatal::failure(format!("a venue withdrawal was not recorded: {e}")))?;
        }
        for venue in self.recorded_withdrawn.difference(&now) {
            self.path
                .record_participation(venue, Participation::Participating, now_ns())
                .map_err(|e| Fatal::failure(format!("a venue admission was not recorded: {e}")))?;
        }
        self.recorded_withdrawn = now;
        Ok(())
    }

    /// The readiness detail, built from this pass's figures so it cannot report halted after an
    /// operator resumed.
    fn standing(&self, valued: &Valuation, evaluation: &Evaluation, answered: usize) -> String {
        let (orders, findings) = self.path.orders();
        format!(
            "{} node; trading {:?}; resuming from proposal {}; took in {} book event(s); {} valued, \
             {} unvalued, {} breach(es), {} unjudged, {:?}; {} venue(s) withdrawn at open, \
             {} on an unverified profile; {} safe-state postcondition(s) answered; {} order(s) \
             may execute, {} unknown awaiting reconciliation, {} lifecycle finding(s); {}",
            self.plan.name(),
            self.path.trading_state(),
            self.path.resume_from(),
            self.market.absorbed,
            valued.snapshot.figures.len(),
            valued.unvalued.len(),
            evaluation.breaches.len(),
            evaluation.unjudged.len(),
            evaluation.freshness,
            self.withdrawn_at_open.len(),
            self.unverified.len(),
            answered,
            orders.may_execute,
            orders.unknown,
            findings,
            self.reconciled,
        )
    }

    /// Judges every waiting proposal against this pass's evaluation. Withdrawn venues are left out
    /// of what is reachable, so no order is built for them.
    async fn answer_proposals(
        &mut self,
        evaluation: &Evaluation,
        withdrawn: &[String],
        standing: &str,
        surface: &Surface,
    ) -> Result<(), Fatal> {
        let mut reachable = Reachable {
            declarations: self
                .adapters
                .iter()
                .filter(|(name, _)| !withdrawn.contains(name))
                .map(|(name, a)| (name.clone(), a.declaration().clone()))
                .collect(),
            adapters: self
                .adapters
                .iter_mut()
                .filter(|(name, _)| !withdrawn.contains(name))
                .map(|(name, a)| (name.clone(), a.as_mut() as &mut dyn VenueExecution))
                .collect(),
        };
        let resolved: BTreeSet<InstrumentId> = self.market.resolved.keys().cloned().collect();
        let judgement = Judgement {
            resolved: &resolved,
            mandates: self.accepted.mandates(),
            evaluation,
            position_held: PositionHeld::No,
            book: &self.market.book,
            staleness_bound_ns: self.accepted.freshness_bound_ns(),
            marks: &self.market.marks,
        };
        let acted = drive(
            &mut self.path,
            &mut reachable,
            clock(),
            &self.store_root,
            self.bound.as_ref().map_or_else(
                || ProposalWindow::whole(streams(self.plan).proposals),
                |b| ProposalWindow {
                    stream: streams(self.plan).proposals,
                    through: b.proposals_through,
                },
            ),
            &judgement,
        )
        .await
        .map_err(|e| Fatal::failure(format!("order path refused: {e}")))?;
        // After the proposals, so a halt driven by this very pass is swept in it.
        crate::withdraw::withdraw_and_flatten(&mut self.path, &mut reachable).await?;
        // Settled in the pass that placed them (decision 0015): an order meets the book it was
        // decided on, then every later update.
        self.paper.settle(now_ns());
        let detail = if acted.is_empty() {
            standing.to_string()
        } else {
            format!("{standing}; answered {} proposal(s)", acted.len())
        };
        surface.report_or_warn(Health::Ready, &detail, now_ns());
        Ok(())
    }
}

/// The order path, opened on a writable store with its limit state replayed and verified.
fn open_path(
    store_root: &std::path::Path,
    plan: RunPlan,
    surface: &Surface,
) -> Result<OrderPath, Fatal> {
    if let Err(e) = bijective_event_store::store::writable(store_root) {
        let detail = format!("the store at {} cannot be written: {e:?}", store_root.display());
        return Err(Fatal::failure(detail));
    }
    // Reported before the replay so a long replay is not read as a hang.
    surface.report_or_warn(Health::Replaying, "rebuilding limit state from the stream", now_ns());

    let named = streams(plan);
    let path = OrderPath::open(store_root, named.control, named.limits)
        .map_err(|e| Fatal::failure(format!("order path refused: {e}")))?;
    match assess(path.limit_recovery()) {
        Startup::Up { detail } => surface.report_or_warn(
            Health::Ready,
            format!("{detail}; resuming from proposal {}", path.resume_from()),
            now_ns(),
        ),
        Startup::Refused { detail } => return Err(Fatal::failure(detail)),
    }
    Ok(path)
}

/// A paper node's simulated venues take back what its record says they held (decision 0018),
/// before the first evidence read, which would otherwise find the record's orders absent.
fn restore_paper(plan: RunPlan, path: &OrderPath, paper: &Paper) {
    if plan.mode() == RunMode::Paper
        && let Some(orders) = path.recorded_orders()
    {
        paper.restore(orders, now_ns());
    }
}

/// States the execution model of a run whose orders meet the simulated venue.
fn announce_simulated(plan: RunPlan) {
    if plan.mode() != RunMode::Live {
        eprintln!(
            "{PROGRAM}: {} orders are matched on the simulated venue: {}",
            plan.name(),
            MODEL.describe()
        );
    }
}
