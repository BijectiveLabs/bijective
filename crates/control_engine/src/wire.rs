// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order path's four class layouts, at version 1.
//! Wire discriminants are pinned constants, never derived from declaration order (as risk limits).

use bijective_authority::{Action, Role};
use bijective_lifecycle::Fill;
use bijective_limits::{
    Amount, Breach, LimitKind, Ratio, SafeState, SafeStateReason, Scope, ScopeLevel, Threshold,
};
use bijective_model::{OrderRequest, Owner, Side, VenueSymbol};
use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_venue::normalise::VenueOutcome;

mod codes;
use codes::*;
mod fill;
use fill::*;
mod adjustment;
pub use adjustment::Adjustment;
use adjustment::*;
mod reconciled;
use reconciled::*;
pub use reconciled::{EvidenceRead, Settlement};
mod encode;
pub use encode::*;
mod decode;
pub use decode::*;

/// The record layout version. 3 adds `VenueOutcome::NotSent` (rule 4, not 5). 4 appends the
/// venue to an intent and adds `OrderFilled`. 5 the owner to an intent. 6 to an adjustment. 7 adds
/// the underlying scope level. 8 the `Uncovered` refusal. 9 `AccountCharged`. 10 appends the order
/// terms to an intent. 11 the stop price. 12 adds `OrderHeld` (0013). 13 appends the expiry (0014).
/// 14 leverage and margin use to a summary (0011). 15 adds `InstrumentScales`. 16 appends the
/// equity to a summary. 17 the platform identity and quote currency to `InstrumentScales`.
pub const VERSION: u16 = 17;

/// The decision identifier of the orders a flatten command places starts with this. A proposal's
/// own identifier never does (the loop refuses one that does), so these orders name no proposal.
pub const FLATTEN_DECISION_PREFIX: &str = "flatten:";

#[must_use]
pub fn flatten_decision_id(command_id: &str) -> String {
    format!("{FLATTEN_DECISION_PREFIX}{command_id}")
}

/// Whether an intent or refusal under `decision_id` acts on a proposal position.
#[must_use]
pub fn names_a_proposal(decision_id: &str) -> bool {
    !decision_id.starts_with(FLATTEN_DECISION_PREFIX)
}

/// Why an order did not reach the venue. Named, because the remedies differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalReason {
    /// The platform was not in a state that admits new orders.
    NotActive { state: SafeState },
    /// The limit state had not been rebuilt, so no budget figure could be stood behind.
    LimitsUnrecovered,
    /// A limit the observed figures exceeded. The action taken is in the `SafeStateDriven` record.
    Breached,
    /// A limit that could not be judged, so the platform does not know whether it is within it.
    Unjudged { kind: LimitKind },
    /// The figures were older than the declared bound.
    Stale { age_ns: i128 },
    /// The figures were stamped after the instant they were judged at.
    ClockSkew { skew_ns: i128 },
    /// A binding scope could not cover what the decision committed.
    NoBudget { scope: Scope, kind: LimitKind },
    /// The proposal never became a decision. Carries desk admission's admission text instead of mirroring its vocabulary.
    NotAdmitted { detail: String },
    /// A proposal this build cannot read. Recorded, never skipped.
    Unreadable,
    /// The venue will not take an order for this instrument, or we cannot say it would.
    InstrumentRefused { symbol: VenueSymbol, cause: InstrumentRefusal },
    /// A reduce-only order that would not reduce: its side does not oppose the held position, or
    /// its quantity with the reduce-only orders already working exceeds it. Signed: negative is short.
    DoesNotReduce { held_scaled: i128, working_scaled: i128 },
    /// The venue's order rate allows no submit before `retry_after_ns` more have passed.
    RateLimited { retry_after_ns: i128 },
    /// A sell of an instrument that cannot be held short, beyond what the record holds there less
    /// the sells already working. Both figures at the instrument's quantity scale.
    Uncovered { held_scaled: i128, working_scaled: i128 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentRefusal {
    /// The venue's catalogue does not carry the symbol.
    Unknown,
    /// Temporarily not accepting orders.
    Halted,
    /// Gone.
    Delisted,
    /// Smaller than the venue's minimum.
    BelowMinimumSize,
}

/// The book as it stood when a strategy decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedBook {
    pub best_bid_ticks: i64,
    pub best_bid_qty_lots: i64,
    pub best_ask_ticks: i64,
    pub best_ask_qty_lots: i64,
    pub observed_at_ns: i128,
    /// `decided_at - observed_at`, non-negative by construction.
    pub capture_lag_ns: i128,
}

/// Whether this node routes to a venue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Participation {
    Participating,
    /// Routed to nothing, and **what is held there is unknown rather than nothing**.
    Withdrawn {
        reason: String,
    },
}

/// One scoped figure, at the money scale. Net keeps its sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeFigure {
    pub scope: Scope,
    pub kind: LimitKind,
    pub value_scaled: i128,
}

/// The portfolio's figures as one pass valued them, at the money scale. A figure that could not be
/// computed is absent, never zero: a book that could not be valued has no gross exposure of zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Valued {
    pub gross_scaled: Option<i128>,
    pub net_scaled: Option<i128>,
    pub daily_loss_scaled: Option<i128>,
    /// The age of the oldest input the figures were made from.
    pub observed_at_ns: i128,
    /// Ratios at `Ratio::ONE`, present only where a leverage or margin-use limit is declared and
    /// the venues stated their margin (decision 0011).
    pub leverage_scaled: Option<i128>,
    pub margin_use_scaled: Option<i128>,
    /// What every venue states backs the account, in the book's currency: cash with positions
    /// at their marks. Absent when a venue is withdrawn or states none. Absent before layout 16.
    pub equity_scaled: Option<i128>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// An order cleared risk and is about to be sent. Written before the venue call, never after,
    /// so a crash leaves an intent that reconciliation can find.
    Intent {
        decision_id: String,
        /// The venue the order was sent to, so a restart knows where to ask about it. `None` only
        /// on intents recorded before version 4, which did not carry it.
        venue: Option<String>,
        /// The desk and strategy the order was placed for, so exposure can be measured per desk
        /// and strategy from its fills. `None` on intents recorded before version 5.
        owner: Option<Owner>,
        request: OrderRequest,
        /// Where in the proposal stream this decision came from, so a restart resumes instead of placing it twice.
        from_position: u64,
        at_ns: i128,
    },
    /// A linked proposal admitted and held until its parent fills (decision 0013), whole, so a
    /// restart still holds it. It acts on the proposal's position as an intent does.
    Held { proposal: bijective_model::Proposal, from_position: u64, at_ns: i128 },
    /// What the venue said about an intent.
    Settled { client_id: String, outcome: VenueOutcome, at_ns: i128 },
    /// A withdrawal was sent to the venue, and what the venue said.
    /// Whether the order's fate stayed open is derived, not stored.
    Withdrawn { client_id: String, instrument: VenueSymbol, outcome: VenueOutcome, at_ns: i128 },
    /// A venue joined or left the set this node routes to, and why. Written on change, never each pass.
    VenueParticipation { venue: String, participation: Participation, at_ns: i128 },
    /// An order did not reach the venue, and why.
    Refused {
        decision_id: String,
        reason: RefusalReason,
        /// Where in the proposal stream this decision came from. A refusal advances the mark like an intent.
        from_position: u64,
        at_ns: i128,
    },
    /// The market when a strategy decided. `book` is absent, not zeroed, when none could be taken.
    Captured {
        correlation_id: String,
        instrument: VenueSymbol,
        decided_at_ns: i128,
        book: Option<CapturedBook>,
        /// Why there is no book. Empty where there is one.
        detail: String,
        from_position: u64,
        at_ns: i128,
    },
    /// A person asked the platform to do something. The state recorded is the one read back.
    CommandApplied {
        command_id: String,
        actor: String,
        role: Role,
        action: Action,
        /// What the trading state was after, read back.
        state: SafeState,
        /// Why, where the command was not carried out.
        detail: String,
        from_position: u64,
        at_ns: i128,
    },
    /// A fill of an order this platform placed, as the venue reported it. Recorded once per
    /// execution identity, so the order and position it moves are rebuilt by replay.
    Filled { client_id: String, fill: Fill, at_ns: i128 },
    /// An order state change venue evidence settled during reconciliation against `venue`.
    Reconciled { venue: String, client_id: String, settlement: Settlement, at_ns: i128 },
    /// The unrealised profit of the whole book at the first complete valuation of UTC day `day`
    /// (days since the epoch), at the money scale. A daily loss is measured from it.
    DayOpened { day: u64, unrealized_scaled: i128, at_ns: i128 },
    /// The portfolio's figures at the money scale, as one pass valued them. Written when they
    /// change, never each pass. Read by the console. Nothing on the order path reads it back.
    PortfolioValued { figures: Valued, at_ns: i128 },
    /// Gross and net exposure at every level below the portfolio (desk, venue, strategy,
    /// underlying, instrument) as one pass valued them, written when they change, never each
    /// pass. A figure the pass could not produce is absent. Read by the console only.
    ScopesValued { figures: Vec<ScopeFigure>, at_ns: i128 },
    /// The highest result UTC day `day` has reached, at the money scale, written each time it
    /// rises. A drawdown is measured from it. Held in the stream so a restart keeps it.
    DayPeak { day: u64, result_scaled: i128, at_ns: i128 },
    /// A charge to `venue`'s account that names no order, as the venue reported it. Recorded
    /// once per charge identifier. The daily loss counts it on the day it is recorded.
    AccountCharged { venue: String, charge: bijective_lifecycle::AccountCharge, at_ns: i128 },
    /// A position change no fill explains, accepted by a risk officer's command.
    PositionAdjusted { adjustment: Adjustment, at_ns: i128 },
    /// The decimals `instrument`'s prices and quantities carry on `venue`, as its profile
    /// declares them: written before the first intent on it, and again if they change, so the
    /// record says what every scaled figure stands for without the profile.
    InstrumentScales {
        venue: String,
        instrument: VenueSymbol,
        price_scale: u8,
        quantity_scale: u8,
        at_ns: i128,
        /// The platform's identity for the instrument, the one market streams carry, and the
        /// currency its prices are stated in. Absent before layout 17.
        identity: Option<String>,
        quote_currency: Option<String>,
    },
    /// What one reconciliation pass against `venue` concluded, one sentence per finding.
    ReconciliationCompleted {
        venue: String,
        verdict: bijective_lifecycle::Verdict,
        findings: Vec<String>,
        evidence: EvidenceRead,
        at_ns: i128,
    },
    /// A safe state was called for. `arrived` is the read-back, not the request.
    SafeStateDriven {
        target: SafeState,
        reason: SafeStateReason,
        arrived: bool,
        at_ns: i128,
        /// What the breach was about. Appended at version 2 because the reason is `Copy` and must not
        /// allocate on the order path. `None` is not "no scope".
        scope: Option<Scope>,
    },
    /// Whether exposure honoured a safe state already driven. Written a pass later.
    SafeStatePostcondition {
        scope: Scope,
        kind: LimitKind,
        /// The figure the drive was raised on, carried from the breach that raised it.
        before: Threshold,
        outcome: PostconditionOutcome,
        at_ns: i128,
    },
}

/// What a later snapshot said about the figure a safe state was driven on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostconditionOutcome {
    /// The figure held or fell.
    Held {
        /// What it stood at when the question was answered.
        after: Threshold,
    },
    /// The figure grew: the instruction was honoured in name and not in effect.
    Increased {
        /// What it had reached.
        after: Threshold,
    },
    /// No comparable figure arrived. Recorded, not silent: a postcondition that stops on missing data hides an unhonoured halt.
    Unverifiable,
}
