// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Event payload encoding - envelope, class ids, and the primitives a layout is written with.
//! Design and rationale: `docs/concepts/event_sourcing.md`.

pub mod reader;
pub mod writer;

pub use reader::{DecodeError, Reader};
pub use writer::Writer;

#[cfg(test)]
#[path = "../../tests/unit/codec/tests.rs"]
mod tests;

/// Longest field this codec will write or read.
pub const MAX_FIELD_LEN: u32 = 1024 * 1024;

/// Every event class the platform emits, with its wire id.
/// Allocated centrally and never reused: two domains sharing an id would decode each other's payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ClassId {
    // 0x01xx - risk limits, risk and limits.
    LimitDeclared = 0x0101,
    LimitConsumed = 0x0102,
    LimitReleased = 0x0103,

    // 0x02xx - the Data Engine.
    MarketBookUpdate = 0x0201,
    LinkTransition = 0x0202,
    /// What the data engine received, journaled before anything acts on it.
    MarketJournalEntry = 0x0203,
    /// A trade, liquidation, funding, open interest, mark, index or long/short figure.
    MarketFact = 0x0204,

    // 0x03xx - the Control Engine's order path.
    OrderIntent = 0x0301,
    OrderSettled = 0x0302,
    OrderRefused = 0x0303,
    SafeStateDriven = 0x0304,
    SafeStatePostcondition = 0x0308,
    /// A fill of an order this platform placed, as the venue reported it.
    OrderFilled = 0x0309,
    /// An order state change that venue evidence settled during reconciliation.
    OrderReconciled = 0x030A,
    /// The outcome of one reconciliation pass against one venue.
    ReconciliationCompleted = 0x030B,
    /// A position change no fill explains, accepted by a risk officer.
    PositionAdjusted = 0x030C,
    /// The first valuation of a UTC day: the baseline a daily loss is measured from.
    DayOpened = 0x030D,
    /// The portfolio's figures as last valued, written when they change, for the readers that show them.
    PortfolioValued = 0x030E,
    /// A new highest result of a UTC day: the point a drawdown is measured from.
    DayPeak = 0x030F,
    /// Exposure at every level below the portfolio, as last valued, for the readers that show it.
    ScopesValued = 0x0310,
    /// A charge to a venue account that names no order, recorded once per charge identifier.
    AccountCharged = 0x0311,
    /// A linked proposal the platform holds until its parent fills (decision 0013).
    OrderHeld = 0x0312,
    /// The scales an instrument's prices and quantities are recorded at on one venue.
    InstrumentScales = 0x0313,
    DecisionCaptured = 0x0305,
    OrderWithdrawn = 0x0306,
    VenueParticipation = 0x0307,

    // 0x05xx - what crosses between the Strategy Engine and the Control Engine.
    ProposalEmitted = 0x0501,
    /// How far a live strategy session was sent through the market stream.
    StrategySessionMark = 0x0502,
    /// What happened to a run, recorded before its output is usable.
    StrategyRunRecorded = 0x0503,
    /// Where a run's proposals landed, and the market it had been sent.
    StrategyRunAdmitted = 0x0504,
    /// What became of a proposal, mirrored from the control stream for the strategy that made it.
    OrderFeedback = 0x0505,

    // 0x07xx - the decision registry's registry: what was claimed, and when it was claimed.
    PreRegistrationDeclared = 0x0701,

    // 0x08xx - venue assurance's curation: what a venue was verified doing, and what it charges.
    VenueAssuranceRecorded = 0x0801,
    VenueFeesDeclared = 0x0802,
    /// The rules a venue sets for each instrument, declared by the data engine from its profile.
    VenueInstrumentsDeclared = 0x0803,

    // 0x06xx - what a person asks the platform to do, and what came of it.
    OperatorCommand = 0x0601,
    CommandApplied = 0x0602,
}

impl ClassId {
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self as u16
    }

    /// Resolve a wire value. `None` is refused by the caller, never skipped.
    #[must_use]
    pub const fn from_u16(value: u16) -> Option<Self> {
        let mut i = 0;
        while i < Self::ALL.len() {
            if Self::ALL[i].as_u16() == value {
                return Some(Self::ALL[i]);
            }
            i += 1;
        }
        None
    }

    /// Every class this build knows, and **the only place that knowledge lives**.
    /// `from_u16` walks it, so a class missing here fails loudly on the first payload instead of silently.
    pub const ALL: [ClassId; 37] = [
        ClassId::LimitDeclared,
        ClassId::LimitConsumed,
        ClassId::LimitReleased,
        ClassId::MarketBookUpdate,
        ClassId::LinkTransition,
        ClassId::MarketJournalEntry,
        ClassId::MarketFact,
        ClassId::OrderIntent,
        ClassId::OrderSettled,
        ClassId::OrderRefused,
        ClassId::SafeStateDriven,
        ClassId::SafeStatePostcondition,
        ClassId::DecisionCaptured,
        ClassId::ProposalEmitted,
        ClassId::StrategySessionMark,
        ClassId::StrategyRunRecorded,
        ClassId::StrategyRunAdmitted,
        ClassId::OrderFeedback,
        ClassId::OperatorCommand,
        ClassId::CommandApplied,
        ClassId::PreRegistrationDeclared,
        ClassId::VenueAssuranceRecorded,
        ClassId::VenueFeesDeclared,
        ClassId::VenueInstrumentsDeclared,
        ClassId::OrderWithdrawn,
        ClassId::VenueParticipation,
        ClassId::OrderFilled,
        ClassId::OrderReconciled,
        ClassId::ReconciliationCompleted,
        ClassId::PositionAdjusted,
        ClassId::DayOpened,
        ClassId::PortfolioValued,
        ClassId::DayPeak,
        ClassId::ScopesValued,
        ClassId::AccountCharged,
        ClassId::OrderHeld,
        ClassId::InstrumentScales,
    ];
}

/// The four bytes at the head of every payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope {
    pub class: ClassId,
    pub version: u16,
}

impl Envelope {
    #[must_use]
    pub const fn new(class: ClassId, version: u16) -> Self {
        Self { class, version }
    }
}

pub const ENVELOPE_LEN: usize = 4;
