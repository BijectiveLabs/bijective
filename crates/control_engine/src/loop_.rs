// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reading proposals and acting on each, in order.
//! Separate from the process so it can be driven without a node.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bijective_common::clock::Clock;
use bijective_event_store::store::read_range;
use bijective_limits::{Evaluation, PositionHeld, ReduceOnlySemantics, SafeState};
use bijective_model::InstrumentId;
use bijective_risk::desks::{CollarMark, MandateBook, admit, within_collar};
use bijective_risk::valuation::Mark;
use bijective_serialization::signal::{Command, Instruction};
use bijective_venue::contract::VenueExecution;
use bijective_venue::profile::CapabilityDeclaration;

use bijective_decision_capture::{Instant, Resolution, SnapshotBuffer};

use crate::path::{Benchmark, Decision, OrderPath, PathError, Placement, Spend};
use crate::wire::CapturedBook;
use crate::wire::RefusalReason;

/// What one proposal came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acted {
    pub position: u64,
    pub placement: Placement,
}

/// Venues this engine can reach. A venue absent here cannot take an order.
pub struct Reachable<'a> {
    pub declarations: BTreeMap<String, CapabilityDeclaration>,
    pub adapters: BTreeMap<String, &'a mut dyn VenueExecution>,
}

/// What one command came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub position: u64,
    pub state: SafeState,
    /// The instruction asked, so the caller that owns the venue set can act on it.
    pub instruction: Instruction,
    /// Authenticated, permitted and new. A refused command is recorded and carried out by nobody.
    pub carried_out: bool,
}

/// Carry out every pending operator command, in order, before any proposal is judged: a halt must
/// not be countermanded by a waiting proposal.
pub fn carry_out_commands(
    path: &mut OrderPath,
    clock: &dyn Clock,
    store_root: &Path,
    commands: &str,
) -> Result<Vec<Carried>, PathError> {
    let from = path.commands_from();
    let entries = read_range(store_root, commands, from, u64::MAX)?;

    let mut carried = Vec::new();
    for entry in entries {
        let Ok((command, signature)) =
            bijective_serialization::signal::command::decode_signed(&entry.payload)
        else {
            // Refused, never skipped, and the mark advances: skipping silently reads as carried out.
            let state = path.carry_out(
                &Command {
                    id: String::new(),
                    actor: String::new(),
                    role: bijective_authority::Role::Observer,
                    instruction: bijective_serialization::signal::Instruction::AcknowledgeAlarm,
                    at_ns: 0,
                },
                None,
                entry.seq,
                clock.now_ns(),
            )?;
            carried.push(Carried {
                position: entry.seq,
                state,
                instruction: Instruction::AcknowledgeAlarm,
                carried_out: false,
            });
            continue;
        };

        let state = path.carry_out(&command, signature.as_ref(), entry.seq, clock.now_ns())?;
        carried.push(Carried {
            position: entry.seq,
            state,
            instruction: command.instruction,
            carried_out: path.last_carried_out(),
        });
    }

    Ok(carried)
}

/// What every proposal in one pass is judged against. A struct so positional arguments cannot be transposed.
pub struct Judgement<'a> {
    pub mandates: &'a MandateBook,
    pub evaluation: &'a Evaluation,
    pub position_held: PositionHeld,
    /// The book, for stamping a decision against what the market was.
    pub book: &'a SnapshotBuffer,
    /// How stale a snapshot may be and still serve as a benchmark.
    pub staleness_bound_ns: i128,
    /// Outcomes whose question resolved (decision 0002, rule 5): they admit no orders.
    pub resolved: &'a BTreeSet<InstrumentId>,
    /// Marks by platform identity, which collar every limit price. Their age is bounded by
    /// `staleness_bound_ns`.
    pub marks: &'a BTreeMap<InstrumentId, Mark>,
}

/// The proposal stream a pass reads and the last position it may act on: `None` reads nothing.
/// A backtest pass reads only what was decided by its step (decision 0015). Every other pass reads
/// to the end.
#[derive(Debug, Clone, Copy)]
pub struct ProposalWindow<'a> {
    pub stream: &'a str,
    pub through: Option<u64>,
}

impl<'a> ProposalWindow<'a> {
    /// Every proposal on `stream` not yet acted on.
    #[must_use]
    pub fn whole(stream: &'a str) -> Self {
        Self { stream, through: Some(u64::MAX) }
    }
}

/// Reads every proposal not yet acted on and acts on each in order: later proposals are judged
/// against the limit state left by earlier ones. Stops at the first path error, so nothing runs on
/// an unrecorded predecessor.
pub async fn drive(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    clock: &dyn Clock,
    store_root: &Path,
    proposals: ProposalWindow<'_>,
    judgement: &Judgement<'_>,
) -> Result<Vec<Acted>, PathError> {
    let from = path.resume_from();
    let entries = match proposals.through {
        Some(through) => read_range(store_root, proposals.stream, from, through)?,
        None => Vec::new(),
    };
    let mut acted = Vec::new();
    for entry in entries {
        let placement =
            act_on(path, reachable, clock, judgement, entry.seq, &entry.payload).await?;
        acted.push(Acted { position: entry.seq, placement });
    }
    // After the new proposals, so a child held this pass is released on the fills already read.
    acted.extend(crate::release::release_held(path, reachable, clock, judgement).await?);
    Ok(acted)
}

/// One proposal: decode, then hold it if it is linked (decision 0013) or place it.
async fn act_on(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    clock: &dyn Clock,
    judgement: &Judgement<'_>,
    seq: u64,
    payload: &[u8],
) -> Result<Placement, PathError> {
    let Ok(proposal) = bijective_serialization::signal::decode(payload) else {
        return path.refuse_unreadable(seq, clock.now_ns());
    };
    if proposal.parent.is_some() || proposal.oco.is_some() {
        return hold(path, reachable, clock, judgement, seq, &proposal);
    }
    place_proposal(path, reachable, clock, judgement, seq, &proposal).await
}

/// Admits a linked proposal and records it held. Desk admission is asked now, so a child that
/// could never be placed is refused while its strategy can still act on the refusal.
fn hold(
    path: &mut OrderPath,
    reachable: &Reachable<'_>,
    clock: &dyn Clock,
    judgement: &Judgement<'_>,
    seq: u64,
    proposal: &bijective_model::Proposal,
) -> Result<Placement, PathError> {
    let at_ns = clock.now_ns();
    if let Some((id, detail)) = before_admission(judgement, proposal, at_ns) {
        return path.refuse_admission(id, seq, detail, at_ns);
    }
    if let Some(detail) = path.unholdable(proposal) {
        return path.refuse_admission(&proposal.client_id, seq, &detail, at_ns);
    }
    let order_id = crate::order_id::client_order_id(at_ns, seq);
    if let Err(refusal) = admit(judgement.mandates, &reachable.declarations, proposal, &order_id) {
        return path.refuse_admission(&proposal.client_id, seq, &format!("{refusal}"), at_ns);
    }
    path.hold(proposal, seq, at_ns)
}

/// One proposal through admission, the collar, a benchmark and the order path. Every refusal is
/// recorded, never skipped.
pub(crate) async fn place_proposal(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    clock: &dyn Clock,
    judgement: &Judgement<'_>,
    seq: u64,
    proposal: &bijective_model::Proposal,
) -> Result<Placement, PathError> {
    if let Some((id, detail)) = before_admission(judgement, proposal, clock.now_ns()) {
        return path.refuse_admission(id, seq, detail, clock.now_ns());
    }
    let order_id = crate::order_id::client_order_id(clock.now_ns(), seq);
    let admitted = match admit(judgement.mandates, &reachable.declarations, proposal, &order_id) {
        Ok(a) => a,
        // The rendered message: an operator reads it out of the control stream.
        Err(refusal) => {
            return path.refuse_admission(
                &proposal.client_id,
                seq,
                &format!("{refusal}"),
                clock.now_ns(),
            );
        }
    };
    if let Err(refusal) = collared(judgement, proposal, clock.now_ns()) {
        return path.refuse_admission(&proposal.client_id, seq, &refusal, clock.now_ns());
    }
    let Ok(spend) = Spend::declare(admitted.spend, admitted.scopes) else {
        return path.refuse_admission(
            &proposal.client_id,
            seq,
            "the admitted spend was not positive or bound to a scope",
            clock.now_ns(),
        );
    };
    let semantics = semantics_of(&reachable.declarations, &admitted.venue);
    let Some(adapter) = reachable.adapters.get_mut(&admitted.venue) else {
        return path.refuse_admission(
            &proposal.client_id,
            seq,
            "no adapter is open for the venue the desk preferred",
            clock.now_ns(),
        );
    };
    let (book, detail) = benchmark(judgement, proposal);
    path.record_capture(
        &Benchmark {
            correlation_id: &proposal.client_id,
            instrument: &admitted.request.instrument,
            decided_at_ns: proposal.decided_at_ns,
            book,
            detail,
        },
        seq,
        clock.now_ns(),
    )?;
    path.place(
        *adapter,
        clock,
        &Decision {
            id: &proposal.client_id,
            venue: &admitted.venue,
            from_position: seq,
            request: &admitted.request,
            spend: &spend,
            evaluation: judgement.evaluation,
            position_held: judgement.position_held,
            semantics,
        },
    )
    .await
}

/// The identifier and text a proposal is refused under before desk admission, or `None`.
fn before_admission<'p>(
    judgement: &Judgement<'_>,
    proposal: &'p bijective_model::Proposal,
    now_ns: i128,
) -> Option<(&'p str, &'static str)> {
    // Judged on this node's clock, as the sweep that cancels at the expiry is (decision 0014).
    if proposal.terms.expire_at_ns.is_some_and(|at| at <= now_ns) {
        return Some((&proposal.client_id, "the proposal expired before it was admitted"));
    }
    if judgement.resolved.contains(&proposal.instrument) {
        let detail = "the outcome's question resolved; it admits no orders (decision 0002, rule 5)";
        return Some((&proposal.client_id, detail));
    }
    // Recorded with no identifier: under its own, the refusal would read as the node's order.
    if !crate::wire::names_a_proposal(&proposal.client_id) {
        return Some((
            "",
            "the proposal identifier starts with a prefix the node keeps for its own",
        ));
    }
    None
}

/// The desk's price collar around the instrument's mark, refused as the text an operator reads.
fn collared(
    judgement: &Judgement<'_>,
    proposal: &bijective_model::Proposal,
    as_of_ns: i128,
) -> Result<(), String> {
    let at = CollarMark {
        mark: judgement.marks.get(&proposal.instrument).copied(),
        as_of_ns,
        max_age_ns: judgement.staleness_bound_ns,
    };
    within_collar(judgement.mandates, proposal, &at).map_err(|refusal| format!("{refusal}"))
}

/// The venue's reduce-only semantics. An undeclared venue is assumed to clamp at fill time,
/// the weaker of the two.
fn semantics_of(
    declarations: &BTreeMap<String, CapabilityDeclaration>,
    venue: &str,
) -> ReduceOnlySemantics {
    declarations.get(venue).map_or(ReduceOnlySemantics::FillTimeClamp, |d| {
        match d.reduce_only() {
            bijective_venue::profile::ReduceOnly::PlacementPrecondition => {
                ReduceOnlySemantics::PlacementPrecondition
            }
            // Unknown venues are assumed to have the weaker reduce-only semantics.
            _ => ReduceOnlySemantics::FillTimeClamp,
        }
    })
}

/// The book at the strategy's decision instant, captured before submission so path latency is
/// not read as market movement. `None` with the cause when no book within the bound exists.
fn benchmark(
    judgement: &Judgement<'_>,
    proposal: &bijective_model::Proposal,
) -> (Option<CapturedBook>, String) {
    // The book is kept under the platform identity the market streams carry, not the venue symbol.
    match judgement.book.capture(
        proposal.instrument.as_str(),
        &proposal.client_id,
        Instant::new(proposal.decided_at_ns, Resolution::Nanosecond),
        judgement.staleness_bound_ns,
    ) {
        Ok(record) => (
            Some(CapturedBook {
                best_bid_ticks: record.book.best_bid_ticks,
                best_bid_qty_lots: record.book.best_bid_qty_lots,
                best_ask_ticks: record.book.best_ask_ticks,
                best_ask_qty_lots: record.book.best_ask_qty_lots,
                observed_at_ns: record.book.observed_at.value_ns,
                capture_lag_ns: record.capture_lag_ns,
            }),
            String::new(),
        ),
        Err(cause) => (None, format!("{cause:?}")),
    }
}

/// The reason recorded when a proposal never became a decision. The admission answer is carried as
/// text so this vocabulary does not duplicate desk admission's.
pub(crate) fn admission_refused(detail: &str) -> RefusalReason {
    RefusalReason::NotAdmitted { detail: detail.to_string() }
}
