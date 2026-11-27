// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the order path is given and what it can answer: the decision, its spend, the placement outcome and the path's errors.

use super::*;

/// Whether the venue will take an order for this instrument. An instrument the catalogue does not
/// carry is refused, not assumed tradable.
pub(crate) fn instrument_refusal(
    declaration: &CapabilityDeclaration,
    request: &OrderRequest,
) -> Option<RefusalReason> {
    let refuse = |cause| {
        Some(RefusalReason::InstrumentRefused { symbol: request.instrument.clone(), cause })
    };

    let Ok(instrument) = declaration.instruments().get(&request.instrument) else {
        return refuse(InstrumentRefusal::Unknown);
    };

    match instrument.status {
        InstrumentStatus::Trading => {}
        InstrumentStatus::Halted => return refuse(InstrumentRefusal::Halted),
        InstrumentStatus::Delisted => return refuse(InstrumentRefusal::Delisted),
    }

    if request.quantity_scaled.abs() < instrument.minimum_size {
        return refuse(InstrumentRefusal::BelowMinimumSize);
    }

    None
}

/// What a decision commits, and where. The path enforces a spend, it never computes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spend {
    pub(crate) amount: Amount,
    pub(crate) scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendError {
    /// Zero or negative: a commitment of nothing is not a commitment.
    NotPositive,
    /// No scope named. An order binds at least the portfolio.
    NoScopes,
}

impl Spend {
    pub fn declare(amount: Amount, scopes: Vec<Scope>) -> Result<Self, SpendError> {
        if amount.scaled() <= 0 {
            return Err(SpendError::NotPositive);
        }
        if scopes.is_empty() {
            return Err(SpendError::NoScopes);
        }
        Ok(Self { amount, scopes })
    }

    #[must_use]
    pub fn amount(&self) -> Amount {
        self.amount
    }

    #[must_use]
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// The desk and strategy the spend binds, which are who the order is placed for. `None`
    /// where it binds no desk.
    #[must_use]
    pub fn owner(&self) -> Option<Owner> {
        let desk = self.scopes.iter().find_map(|s| match s {
            Scope::Desk(name) => Some(name.clone()),
            _ => None,
        })?;
        let strategy = self.scopes.iter().find_map(|s| match s {
            Scope::Strategy(name) => Some(name.clone()),
            _ => None,
        });
        Some(Owner { desk, strategy })
    }
}

/// One decision, as the order path receives it. A struct so positional arguments cannot be transposed.
#[derive(Debug, Clone, Copy)]
pub struct Decision<'a> {
    pub id: &'a str,
    /// The venue the order goes to. Recorded with the intent.
    pub venue: &'a str,
    /// Where in the proposal stream this decision came from, so a restart resumes rather than replays.
    pub from_position: u64,
    pub request: &'a OrderRequest,
    pub spend: &'a Spend,
    pub evaluation: &'a Evaluation,
    pub position_held: PositionHeld,
    pub semantics: ReduceOnlySemantics,
}

/// What happened to one order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// It reached the venue. Carries the outcome, not a boolean: unknown is an answer.
    Sent { outcome: VenueOutcome },
    /// It did not reach the venue.
    Refused { reason: RefusalReason },
    /// A linked proposal, held until its parent fills (decision 0013).
    Held,
}

/// What happened to one withdrawal. `unresolved` applies `needs_reconciliation` so a caller cannot forget to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withdrawal {
    pub outcome: VenueOutcome,
    /// The venue refused, and did not say whether the order is gone because it filled.
    pub unresolved: bool,
}

/// Why the path could not do its job at all. A refusal is the path working. This means the record
/// could not be written, and a path that cannot write what it did must not act.
#[derive(Debug)]
pub enum PathError {
    Store(StoreError),
    Encode(bijective_serialization::codec::writer::EncodeError),
    /// The limit state could not be rebuilt. Carried as text to keep the replay's stream position.
    Replay(String),
    /// Budget was cleared, then could not be written. The order does not go. The residue is conservative
    /// and reconciliation can release it.
    Consume(StateError),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::Store(e) => write!(f, "store: {e}"),
            PathError::Encode(e) => write!(f, "encode: {e}"),
            PathError::Replay(e) => write!(f, "limit-state replay refused: {e}"),
            PathError::Consume(e) => write!(f, "consumption refused after budget cleared: {e}"),
        }
    }
}

impl std::error::Error for PathError {}

impl From<StoreError> for PathError {
    fn from(e: StoreError) -> Self {
        PathError::Store(e)
    }
}

/// How restrictive a state is. Duplicated from `trading` on purpose: the two uses are unrelated.
pub(crate) const fn severity(state: SafeState) -> u8 {
    match state {
        SafeState::Active => 0,
        SafeState::Reducing => 1,
        SafeState::Halted => 2,
    }
}

/// What the market was when a strategy decided, ready to be recorded.
#[derive(Debug, Clone)]
pub struct Benchmark<'a> {
    pub correlation_id: &'a str,
    pub instrument: &'a VenueSymbol,
    pub decided_at_ns: i128,
    /// Absent where none could be taken. `detail` says why.
    pub book: Option<CapturedBook>,
    pub detail: String,
}
