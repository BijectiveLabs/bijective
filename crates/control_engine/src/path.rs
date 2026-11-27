// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order path: the only route from a decision to a venue.
//! See `docs/concepts/order_path.md`, the platform architecture and risk limits.

use std::path::Path;

use bijective_common::clock::Clock;
use bijective_event_store::projection::Recovered;
use bijective_event_store::store::{StoreError, StreamWriter};
use bijective_limits::{
    Amount, DriveFailure, Evaluation, ExposureSnapshot, Freshness, LimitKind, PositionHeld,
    ReduceOnlySemantics, SafeState, SafeStateReason, Scope, Threshold, drive_safe_state,
    select_safe_state, verify_exposure_did_not_increase,
};
use bijective_model::{OrderRequest, Owner, VenueSymbol};
use bijective_risk::limit_state::{Budget, LimitState, ReleaseReason, StateError};
use bijective_venue::contract::VenueExecution;
use bijective_venue::normalise::{Attempted, VenueOutcome};
use bijective_venue::profile::{CapabilityDeclaration, InstrumentStatus};

use crate::clear_to_submit;
use crate::replay::Replayed;
use crate::trading::TradingState;
use crate::wire::{
    CapturedBook, ControlEvent, InstrumentRefusal, Participation, PostconditionOutcome,
    RefusalReason, encode,
};

mod adjust;
mod commands;
mod decision;
mod fills;
mod flatten;
mod held;
mod reconcile;
mod reduce;
pub use reduce::enforces_reduce_only;
mod reservations;
mod scales;
mod summary;
mod throttle;
pub use decision::*;
pub use fills::FillRecorded;
pub use flatten::FlattenOrder;
pub use reconcile::{EVIDENCE_CLOCK_MARGIN_NS, Reconciled};

/// The Control Engine's order path. The clearance never leaves this type, so no caller can reach a
/// venue without a cleared evaluation, or without the attempt being written down first.
pub struct OrderPath {
    trading: TradingState,
    journal: StreamWriter,
    limits: LimitState,
    limit_journal: StreamWriter,
    replayed: Recovered<Replayed>,
    /// Per venue. A venue with none set is not bounded here, and the node sets one for every
    /// venue its configuration declares, which refuses a venue without one.
    rates: std::collections::BTreeMap<String, crate::rate::RateWindow>,
    /// Who may be believed. Absent means the role a command claims stands, which the node permits
    /// outside live mode only.
    operators: Option<bijective_identity::OperatorRegistry>,
    /// The portfolio summary last recorded, and when.
    last_valued: Option<(crate::wire::Valued, i128)>,
    /// The scoped figures last recorded and when. Held like `last_valued`.
    last_scopes: Option<(Vec<crate::wire::ScopeFigure>, i128)>,
    /// What became of each proposal, mirrored for the strategies that made them.
    mirror: crate::feedback::Mirror,
    /// Venues whose latest pass saw an order filled ahead of its fills: read again on the next
    /// pass rather than the next interval. Held in memory. A start reads every venue anyway.
    awaiting_fills: std::collections::BTreeSet<String>,
    /// Whether the latest command was carried out rather than refused.
    carried_out: bool,
    /// Positions a flatten order was refused for in this process, by venue and symbol.
    flatten_refused: std::collections::BTreeSet<(String, VenueSymbol)>,
    /// Held proposals whose release was refused in this process, by identifier.
    release_refused: std::collections::BTreeSet<String>,
}

impl OrderPath {
    /// Rebuild the limit state, take both writers, and start halted. The path owns the ledger so the
    /// single-writer lock cannot be decided by who opens first.
    pub fn open(root: &Path, control: &str, limits: &str) -> Result<Self, PathError> {
        let state =
            LimitState::recover(root, limits).map_err(|e| PathError::Replay(e.to_string()))?;

        // Read before the writer is taken, so the stream is not one this process could already have appended to.
        let replayed = Recovered::<Replayed>::rebuild(root, control)
            .map_err(|e| PathError::Replay(e.to_string()))?;

        let mut path = Self {
            trading: TradingState::new(),
            journal: StreamWriter::open(root, control)?,
            // After the writer is taken: completes the mirror from a stream nothing else now appends to.
            mirror: crate::feedback::Mirror::open(root, control)?,
            limits: state,
            limit_journal: StreamWriter::open(root, limits)?,
            replayed,
            rates: std::collections::BTreeMap::new(),
            awaiting_fills: std::collections::BTreeSet::new(),
            operators: None,
            last_valued: None,
            last_scopes: None,
            carried_out: false,
            flatten_refused: std::collections::BTreeSet::new(),
            release_refused: std::collections::BTreeSet::new(),
        };
        // Repairs a crash between an order finishing and its reservation coming back.
        path.settle_reservations()?;
        Ok(path)
    }

    #[must_use]
    pub fn trading_state(&self) -> SafeState {
        self.trading.current()
    }

    /// The first proposal position not acted on. Zero if the rebuild is incomplete, which is the safe
    /// side: reading a proposal again is better than acting on it twice.
    #[must_use]
    pub fn resume_from(&self) -> u64 {
        self.replayed.state().map_or(0, |r| r.progress().resume_from())
    }

    #[must_use]
    pub fn limit_recovery(&self) -> bijective_risk::limit_state::Recovery {
        self.limits.recovery()
    }

    /// The limit ledger, for a caller that declares limits before trading opens.
    pub fn limits_mut(&mut self) -> (&mut LimitState, &mut StreamWriter) {
        (&mut self.limits, &mut self.limit_journal)
    }

    /// Return the platform to a less restrictive state. An operator's action, never the path's.
    pub fn resume(&mut self, state: SafeState) {
        self.trading.resume(state);
    }

    /// The order record as replayed. `None` when no history could be replayed.
    #[must_use]
    pub fn recorded_orders(&self) -> Option<&crate::orders::Orders> {
        self.replayed.state().map(Replayed::orders)
    }

    /// The unrealised profit recorded when UTC day `day` opened, if it has.
    #[must_use]
    pub fn day_baseline(&self, day: u64) -> Option<i128> {
        self.replayed.state().and_then(|r| r.daily().baseline_for(day))
    }

    /// The highest result recorded for UTC day `day`, if any.
    #[must_use]
    pub fn day_peak(&self, day: u64) -> Option<i128> {
        self.replayed.state().and_then(|r| r.daily().peak_for(day))
    }

    /// Record a new highest result of a day. A lower one changes nothing on replay.
    pub fn raise_peak(
        &mut self,
        day: u64,
        result_scaled: i128,
        at_ns: i128,
    ) -> Result<(), PathError> {
        self.record(&ControlEvent::DayPeak { day, result_scaled, at_ns })?;
        Ok(())
    }

    /// Record the baseline of a day. The first record of a day stands.
    pub fn open_day(
        &mut self,
        day: u64,
        unrealized_scaled: i128,
        at_ns: i128,
    ) -> Result<(), PathError> {
        self.record(&ControlEvent::DayOpened { day, unrealized_scaled, at_ns })?;
        Ok(())
    }

    /// The order lifecycle as recorded: what may still execute, what is unknown, and how many
    /// records the state machine refused.
    pub fn orders(&self) -> (bijective_lifecycle::LedgerSummary, usize) {
        self.replayed.state().map_or((bijective_lifecycle::LedgerSummary::default(), 0), |r| {
            (r.orders().summary(), r.orders().findings().len())
        })
    }

    /// Why resuming is refused, or `None`. An unknown order may have executed, and a diverged
    /// venue holds what the ledger does not: either way the position traded on is not known.
    fn resume_barrier(&self) -> Option<String> {
        let unknown = self.orders().0.unknown;
        if unknown > 0 {
            return Some(format!("{unknown} order(s) unknown; resume refused until reconciled"));
        }
        let orders = self.replayed.state().map(Replayed::orders);
        let unread: Vec<&str> = orders.map(|o| o.unread().collect()).unwrap_or_default();
        if !unread.is_empty() {
            return Some(format!(
                "evidence from {} could not be read; resume refused until it is",
                unread.join(", ")
            ));
        }
        let diverged: Vec<&str> = orders.map(|o| o.diverged().collect()).unwrap_or_default();
        (!diverged.is_empty()).then(|| {
            format!(
                "reconciliation diverged on {}; resume refused until a pass agrees",
                diverged.join(", ")
            )
        })
    }

    /// The first command position this engine has not carried out.
    #[must_use]
    pub fn commands_from(&self) -> u64 {
        self.replayed.state().map_or(0, |r| r.progress().commands_from())
    }

    /// Append to the control stream and advance the engine's own mark with it, so the next pass does
    /// not place the same orders again.
    fn record(&mut self, event: &ControlEvent) -> Result<u64, PathError> {
        let payload = encode(event).map_err(PathError::Encode)?;
        let seq = self.journal.append(&payload)?;
        self.replayed.advance(event).map_err(|e| PathError::Replay(format!("{e:?}")))?;
        self.mirror.observe(seq, event)?;
        Ok(seq)
    }

    /// Drive a safe state, read back whether it took hold, and record both. Skipped when the platform
    /// is already at least as restrictive, so a safer-than-requested state raises no false alarm.
    fn escalate(
        &mut self,
        target: SafeState,
        reason: SafeStateReason,
        scope: Option<&Scope>,
        at_ns: i128,
    ) -> Result<(), PathError> {
        let arrived = if severity(target) <= severity(self.trading.current()) {
            true
        } else {
            drive_safe_state(&mut self.trading, target).is_ok()
        };

        self.record(&ControlEvent::SafeStateDriven {
            target,
            reason,
            arrived,
            at_ns,
            scope: scope.cloned(),
        })?;
        Ok(())
    }

    /// Answer every safe state waiting on a later view of its figure. Not done inside `escalate`: it
    /// needs fresh venue data, a network round trip on a latency path. Returns the count answered.
    pub fn answer_postconditions(
        &mut self,
        after: &ExposureSnapshot,
        at_ns: i128,
    ) -> Result<usize, PathError> {
        // Collected first: recording an answer advances the projection this list came from.
        let awaiting: Vec<(Scope, LimitKind, Threshold)> = self
            .replayed
            .state()
            .map(|r| {
                r.unverified()
                    .awaiting()
                    .map(|(scope, kind, before)| (scope.clone(), kind, before))
                    .collect()
            })
            .unwrap_or_default();

        let mut answered = 0;
        for (scope, kind, before) in awaiting {
            let outcome = match verify_exposure_did_not_increase(before, after, &scope, kind) {
                Ok(observed) => PostconditionOutcome::Held { after: observed },
                Err(DriveFailure::ExposureIncreasedAfterSafeState { after, .. }) => {
                    PostconditionOutcome::Increased { after }
                }
                // Missing figure, incomparable units or a drive-half failure: none is the postcondition satisfied.
                // An outcome rather than a panic, so a rule change cannot stop an operator's process.
                Err(_) => PostconditionOutcome::Unverifiable,
            };

            self.record(&ControlEvent::SafeStatePostcondition {
                scope,
                kind,
                before,
                outcome,
                at_ns,
            })?;
            answered += 1;
        }

        Ok(answered)
    }

    /// Record a refusal against an identifier and a position. Not a `Decision`: a proposal refused
    /// before admission has no order, spend or evaluation to record.
    fn refuse(
        &mut self,
        id: &str,
        from_position: u64,
        reason: RefusalReason,
        at_ns: i128,
    ) -> Result<Placement, PathError> {
        self.record(&ControlEvent::Refused {
            decision_id: id.to_string(),
            reason: reason.clone(),
            from_position,
            at_ns,
        })?;
        Ok(Placement::Refused { reason })
    }

    /// Record a proposal this build could not read. Refused, not skipped, and the mark advances.
    pub fn refuse_unreadable(
        &mut self,
        position: u64,
        at_ns: i128,
    ) -> Result<Placement, PathError> {
        // No identifier: an unreadable payload has none we could stand behind.
        self.refuse("", position, RefusalReason::Unreadable, at_ns)
    }

    /// Record a proposal that did not pass desk admission's admission.
    pub fn refuse_admission(
        &mut self,
        id: &str,
        position: u64,
        detail: &str,
        at_ns: i128,
    ) -> Result<Placement, PathError> {
        self.refuse(id, position, crate::loop_::admission_refused(detail), at_ns)
    }

    /// Record that a venue joined or left the set. Called on change only. The caller holds the previous belief.
    pub fn record_participation(
        &mut self,
        venue: &str,
        participation: Participation,
        at_ns: i128,
    ) -> Result<(), PathError> {
        self.record(&ControlEvent::VenueParticipation {
            venue: venue.to_string(),
            participation,
            at_ns,
        })?;
        Ok(())
    }

    /// Record the market when a strategy decided, even with no benchmark, so execution quality attribution can see the gap.
    pub fn record_capture(
        &mut self,
        benchmark: &Benchmark<'_>,
        position: u64,
        at_ns: i128,
    ) -> Result<(), PathError> {
        self.record(&ControlEvent::Captured {
            correlation_id: benchmark.correlation_id.to_string(),
            instrument: benchmark.instrument.clone(),
            decided_at_ns: benchmark.decided_at_ns,
            book: benchmark.book,
            detail: benchmark.detail.clone(),
            from_position: position,
            at_ns,
        })?;
        Ok(())
    }

    /// The first reason the order may not be judged by risk at all, in order: the trading state,
    /// unrecovered limits, an instrument the venue will not take, a reduction that does not reduce.
    fn admission_refusal(
        &self,
        declaration: &CapabilityDeclaration,
        decision: &Decision<'_>,
    ) -> Option<RefusalReason> {
        // A reduction is the one order a reducing platform takes, and it still needs a clearance.
        let reduction =
            decision.request.reduce_only && self.trading.current() == SafeState::Reducing;
        if !self.trading.admits_new_orders() && !reduction {
            return Some(RefusalReason::NotActive { state: self.trading.current() });
        }
        if !self.limits.may_trade() {
            return Some(RefusalReason::LimitsUnrecovered);
        }
        // Before risk: an instrument the venue will not take is not a risk question, and a halt
        // over one would be misattributed.
        if let Some(reason) = instrument_refusal(declaration, decision.request) {
            return Some(reason);
        }
        // Judged on the record: a reduce-only order that would open or grow a position is
        // refused whatever the limits say.
        if decision.request.reduce_only {
            return self.does_not_reduce(
                declaration,
                decision.venue,
                decision.request,
                decision.id,
            );
        }
        self.uncovered(declaration, decision.venue, decision.request, decision.id)
    }

    /// Run one decision through risk and, if it clears, to the venue. A halted platform is refused
    /// before limits are consulted: a fresh evaluation must not talk the platform out of a halt.
    pub async fn place(
        &mut self,
        venue: &mut dyn VenueExecution,
        clock: &dyn Clock,
        decision: &Decision<'_>,
    ) -> Result<Placement, PathError> {
        let at_ns = clock.now_ns();

        if let Some(reason) = self.admission_refusal(venue.declaration(), decision) {
            return self.refuse(decision.id, decision.from_position, reason, at_ns);
        }

        let Some(clearance) = clear_to_submit(decision.evaluation, at_ns) else {
            let reason = self.act_on(decision, at_ns)?;
            return self.refuse(decision.id, decision.from_position, reason, at_ns);
        };

        // After risk, so a breach is still acted on. Before any budget is drawn or intent written.
        if let Some(reason) = self.rate_refusal(decision.venue, at_ns) {
            return self.refuse(decision.id, decision.from_position, reason, at_ns);
        }

        //. Reserving against a reduce-only order would refuse risk-reducing orders near the limit.
        let draws = !decision.request.reduce_only;
        if draws {
            if let Some(reason) = self.short_of_budget(decision.spend) {
                return self.refuse(decision.id, decision.from_position, reason, at_ns);
            }
            self.consume(decision)?;
        }

        // Written before the call, never after: a crash then leaves an intent reconciliation can find.
        self.declare_scales(
            decision.venue,
            venue.declaration(),
            &decision.request.instrument,
            at_ns,
        )?;
        self.record(&ControlEvent::Intent {
            decision_id: decision.id.to_string(),
            venue: Some(decision.venue.to_string()),
            owner: decision.spend.owner(),
            request: decision.request.clone(),
            from_position: decision.from_position,
            at_ns,
        })?;

        let outcome = venue.submit(&clearance, decision.request, at_ns).await;
        self.count_submit(decision.venue, &outcome, at_ns);

        self.record(&ControlEvent::Settled {
            client_id: decision.request.client_id.clone(),
            outcome: outcome.clone(),
            at_ns: clock.now_ns(),
        })?;

        // Only a rejection that leaves no order returns the reservation: an unknown may have
        // filled, and a duplicate says the order is already at the venue.
        if draws && outcome.leaves_no_order() {
            self.release(decision)?;
        }

        Ok(Placement::Sent { outcome })
    }

    /// Withdraw a resting order. No clearance and no budget returned: a platform in breach must still be
    /// able to cancel. One record, after the call, since a cancel cannot move capital.
    pub async fn withdraw(
        &mut self,
        venue: &mut dyn VenueExecution,
        instrument: &VenueSymbol,
        client_id: &str,
        at_ns: i128,
    ) -> Result<Withdrawal, PathError> {
        let outcome = venue.cancel(instrument, client_id, at_ns).await;

        self.record(&ControlEvent::Withdrawn {
            client_id: client_id.to_string(),
            instrument: instrument.clone(),
            outcome: outcome.clone(),
            at_ns,
        })?;

        Ok(Withdrawal { unresolved: outcome.needs_reconciliation(Attempted::Cancel), outcome })
    }

    /// Whether any binding scope cannot cover the spend. Checked across all scopes before any write,
    /// so 4c's all-or-nothing rule does not depend on scope order.
    fn short_of_budget(&self, spend: &Spend) -> Option<RefusalReason> {
        for scope in spend.scopes() {
            match self.limits.budget(scope, LimitKind::GrossExposure) {
                // Undeclared or ratio: no allowance to draw against, which is not an allowance of zero.
                Budget::Undeclared | Budget::NotDrawable => {}
                Budget::Remaining(left) if left.scaled() >= spend.amount().scaled() => {}
                _ => {
                    return Some(RefusalReason::NoBudget {
                        scope: scope.clone(),
                        kind: LimitKind::GrossExposure,
                    });
                }
            }
        }
        None
    }

    fn draws_against(&self, scope: &Scope) -> bool {
        !matches!(
            self.limits.budget(scope, LimitKind::GrossExposure),
            Budget::Undeclared | Budget::NotDrawable
        )
    }

    fn consume(&mut self, decision: &Decision<'_>) -> Result<(), PathError> {
        for scope in decision.spend.scopes() {
            if !self.draws_against(scope) {
                continue;
            }
            self.limits
                .consume(
                    &mut self.limit_journal,
                    scope.clone(),
                    LimitKind::GrossExposure,
                    decision.spend.amount(),
                    decision.id,
                )
                .map_err(PathError::Consume)?;
        }
        Ok(())
    }

    fn release(&mut self, decision: &Decision<'_>) -> Result<(), PathError> {
        for scope in decision.spend.scopes() {
            if !self.draws_against(scope) {
                continue;
            }
            self.limits
                .release(
                    &mut self.limit_journal,
                    scope.clone(),
                    LimitKind::GrossExposure,
                    decision.spend.amount(),
                    ReleaseReason::OrderCancelled,
                    Some(decision.id),
                )
                .map_err(PathError::Consume)?;
        }
        Ok(())
    }

    /// Take the safe-state action an uncleared evaluation calls for. Each breach drives and records
    /// its own state. `TradingState::set` escalates to the heaviest.
    fn act_on(&mut self, decision: &Decision<'_>, at_ns: i128) -> Result<RefusalReason, PathError> {
        let evaluation = decision.evaluation;
        if !evaluation.breaches.is_empty() {
            for scoped in &evaluation.breaches {
                let target =
                    select_safe_state(&scoped.breach, decision.position_held, decision.semantics);
                self.escalate(
                    target,
                    SafeStateReason::Breached(scoped.breach),
                    Some(&scoped.scope),
                    at_ns,
                )?;
            }
            return Ok(RefusalReason::Breached);
        }

        // An unjudged limit is not a passed limit: it stops the order, and the reason is recorded.
        if let Some(kind) = evaluation.unjudged.first().copied() {
            for unjudged in &evaluation.unjudged {
                // No scope: nothing was observed to name.
                self.escalate(
                    SafeState::Halted,
                    SafeStateReason::Unjudgeable { kind: *unjudged },
                    None,
                    at_ns,
                )?;
            }
            return Ok(RefusalReason::Unjudged { kind });
        }

        // Stale figures refuse the order and drive nothing. The pre-trade check owns the staleness bound.
        match evaluation.freshness {
            Freshness::Stale { age_ns } => Ok(RefusalReason::Stale { age_ns }),
            Freshness::Implausible { skew_ns } => Ok(RefusalReason::ClockSkew { skew_ns }),
            // Unreachable today. A refusal rather than `unreachable!()` so a mint rule change fails closed.
            Freshness::Fresh => Ok(RefusalReason::LimitsUnrecovered),
        }
    }
}
