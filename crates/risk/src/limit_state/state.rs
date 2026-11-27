// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Consumed budget, as a replay. Rebuilt from the store on every start, never persisted
//! alongside it. See `docs/concepts/risk_and_limits.md`.

use std::collections::BTreeMap;
use std::path::Path;

use bijective_event_store::projection::{AdvanceError, Projection, RebuildError, Recovered};
use bijective_event_store::store::{StoreError, StreamWriter};
use bijective_limits::{Amount, ArithmeticError, LimitKind, Scope, Threshold};

/// Re-exported: the core owns what recovered means.
pub use bijective_event_store::projection::Recovery;

use crate::limit_state::wire::{LimitEvent, ReleaseReason, decode, encode};

/// What is left to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// Not zero and not an error: the caller must not trade on it.
    Unrecovered,
    Undeclared,
    NotDrawable,
    Remaining(Amount),
    Exhausted,
}

#[derive(Debug)]
pub enum StateError {
    Arithmetic(ArithmeticError),
    /// Refused, not clamped: clamping would let one of two conflicting stories win quietly.
    ReleaseExceedsConsumed {
        scope: Scope,
        kind: LimitKind,
    },
    /// Appending would spend against a balance that was never read.
    NotRecovered,
    Store(StoreError),
    Decode(bijective_serialization::codec::DecodeError),
    Encode(bijective_serialization::codec::writer::EncodeError),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Arithmetic(_) => write!(f, "amount not representable"),
            StateError::ReleaseExceedsConsumed { scope, kind } => {
                write!(f, "a release for {scope:?}/{kind:?} exceeds what was consumed")
            }
            StateError::NotRecovered => {
                write!(f, "the limit state has not been rebuilt; nothing may be written")
            }
            StateError::Store(e) => write!(f, "store: {e}"),
            StateError::Decode(e) => write!(f, "decode: {e}"),
            StateError::Encode(e) => write!(f, "encode: {e}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<StoreError> for StateError {
    fn from(e: StoreError) -> Self {
        StateError::Store(e)
    }
}

type Key = (Scope, LimitKind);

/// One decision's reservation in one scope: what it took, and what of that it still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holding {
    pub taken: Amount,
    pub held: Amount,
}

/// The fold with no opinion about recovery. Callers trade only through `LimitState`.
#[derive(Debug, Default)]
pub struct Limits {
    declared: BTreeMap<Key, Threshold>,
    consumed: BTreeMap<Key, Amount>,
    /// What each decision took and still holds, per scope, so a finished order returns exactly
    /// its own share.
    /// Keyed by decision first, so one decision's holdings are read without scanning the others.
    held_by_decision: BTreeMap<String, BTreeMap<Key, Holding>>,
}

impl Projection for Limits {
    type Event = LimitEvent;
    type Error = StateError;

    fn decode(payload: &[u8]) -> Result<LimitEvent, StateError> {
        decode(payload).map_err(StateError::Decode)
    }

    /// Planned in full before anything moves, so a refused event leaves the state as it was.
    fn apply(&mut self, event: &LimitEvent) -> Result<(), StateError> {
        let change = self.plan(event)?;
        self.commit(change);
        Ok(())
    }
}

/// What one event does to the fold, computed without changing it.
enum Change {
    Declare(Key, Threshold),
    Spend { key: Key, total: Amount, decision: Option<((String, Key), Holding)> },
}

impl Limits {
    /// The event's effect, or why it is refused. Pure, so the writer can ask before it writes.
    fn plan(&self, event: &LimitEvent) -> Result<Change, StateError> {
        match event {
            LimitEvent::Declared { scope, kind, maximum, .. } => {
                Ok(Change::Declare((scope.clone(), *kind), *maximum))
            }
            LimitEvent::Consumed { scope, kind, amount, decision_id } => {
                let key = (scope.clone(), *kind);
                let total = self.total(&key).add(*amount).map_err(StateError::Arithmetic)?;
                let by = (decision_id.clone(), key.clone());
                let mine = self.holding(&by);
                let taken = mine.taken.add(*amount).map_err(StateError::Arithmetic)?;
                let held = mine.held.add(*amount).map_err(StateError::Arithmetic)?;
                Ok(Change::Spend { key, total, decision: Some((by, Holding { taken, held })) })
            }
            LimitEvent::Released { scope, kind, amount, decision_id, .. } => {
                let key = (scope.clone(), *kind);
                let refused =
                    || StateError::ReleaseExceedsConsumed { scope: scope.clone(), kind: *kind };
                let total = self.total(&key).sub(*amount).map_err(StateError::Arithmetic)?;
                if total.scaled() < 0 {
                    return Err(refused());
                }
                // A decision returns no more than it took: the excess would be budget nobody reserved.
                let decision = match decision_id {
                    None => None,
                    Some(id) => {
                        let by = (id.clone(), key.clone());
                        let mine = self.holding_of(&by).ok_or_else(refused)?;
                        let held = mine.held.sub(*amount).map_err(StateError::Arithmetic)?;
                        if held.scaled() < 0 {
                            return Err(refused());
                        }
                        Some((by, Holding { held, ..mine }))
                    }
                };
                Ok(Change::Spend { key, total, decision })
            }
        }
    }

    fn commit(&mut self, change: Change) {
        match change {
            Change::Declare(key, maximum) => {
                self.declared.insert(key, maximum);
            }
            Change::Spend { key, total, decision } => {
                self.consumed.insert(key, total);
                if let Some((by, holding)) = decision {
                    let (decision, key) = by;
                    self.held_by_decision.entry(decision).or_default().insert(key, holding);
                }
            }
        }
    }

    fn total(&self, key: &Key) -> Amount {
        self.consumed.get(key).copied().unwrap_or(Amount::from_scaled(0))
    }

    fn holding(&self, by: &(String, Key)) -> Holding {
        let zero = Amount::from_scaled(0);
        self.holding_of(by).unwrap_or(Holding { taken: zero, held: zero })
    }

    fn holding_of(&self, (decision, key): &(String, Key)) -> Option<Holding> {
        self.held_by_decision.get(decision)?.get(key).copied()
    }

    /// No recovery check: holding a rebuilt projection is the guarantee.
    fn budget(&self, scope: &Scope, kind: LimitKind) -> Budget {
        let key = (scope.clone(), kind);
        let Some(maximum) = self.declared.get(&key) else {
            return Budget::Undeclared;
        };
        // Ratio limits are not drawn down. A number here would mix units.
        let Threshold::Money(maximum) = maximum else {
            return Budget::NotDrawable;
        };
        let spent = self.consumed.get(&key).copied().unwrap_or(Amount::from_scaled(0));
        match maximum.sub(spent) {
            Ok(left) if left.scaled() > 0 => Budget::Remaining(left),
            Ok(_) => Budget::Exhausted,
            // An unrepresentable remainder is not a reason to permit a spend.
            Err(_) => Budget::Exhausted,
        }
    }
}

/// The fold plus the evidence it was rebuilt, so one cannot be read without the other.
#[derive(Debug, Default)]
pub struct LimitState {
    held: Recovered<Limits>,
}

impl LimitState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn recovery(&self) -> Recovery {
        self.held.recovery()
    }

    /// One method so no call site writes the comparison the wrong way round.
    #[must_use]
    pub fn may_trade(&self) -> bool {
        self.held.state().is_some()
    }

    pub fn budget(&self, scope: &Scope, kind: LimitKind) -> Budget {
        match self.held.state() {
            None => Budget::Unrecovered,
            Some(limits) => limits.budget(scope, kind),
        }
    }

    /// Fails closed: an event this build cannot read is never skipped. Errors carry the position.
    pub fn recover(root: &Path, stream: &str) -> Result<Self, RebuildError<StateError>> {
        Recovered::<Limits>::rebuild(root, stream).map(|held| Self { held })
    }

    /// Checked, then written, then applied: an event the replay would refuse never reaches the
    /// stream, where it would stop every later start.
    fn record(&mut self, writer: &mut StreamWriter, event: LimitEvent) -> Result<u64, StateError> {
        self.held.state().ok_or(StateError::NotRecovered)?.plan(&event)?;
        let payload = encode(&event).map_err(StateError::Encode)?;
        let seq = writer.append(&payload)?;
        self.held.advance(&event).map_err(|e| match e {
            AdvanceError::NotRecovered => StateError::NotRecovered,
            AdvanceError::Rejected(cause) => cause,
        })?;
        Ok(seq)
    }

    /// Write first, state second: the reverse lets a restart restore budget already spent.
    pub fn consume(
        &mut self,
        writer: &mut StreamWriter,
        scope: Scope,
        kind: LimitKind,
        amount: Amount,
        decision_id: &str,
    ) -> Result<u64, StateError> {
        self.record(
            writer,
            LimitEvent::Consumed { scope, kind, amount, decision_id: decision_id.to_string() },
        )
    }

    /// `decision_id` names the order whose reservation comes back. `None` for budget no single
    /// order holds.
    pub fn release(
        &mut self,
        writer: &mut StreamWriter,
        scope: Scope,
        kind: LimitKind,
        amount: Amount,
        reason: ReleaseReason,
        decision_id: Option<&str>,
    ) -> Result<u64, StateError> {
        let decision_id = decision_id.map(str::to_string);
        self.record(writer, LimitEvent::Released { scope, kind, amount, reason, decision_id })
    }

    /// What is consumed in one scope and kind across every decision. Zero before the rebuild.
    #[must_use]
    pub fn consumed(&self, scope: &Scope, kind: LimitKind) -> Amount {
        self.held
            .state()
            .and_then(|l| l.consumed.get(&(scope.clone(), kind)).copied())
            .unwrap_or(Amount::from_scaled(0))
    }

    /// What `decision_id` took and still holds, per scope and kind. Empty for a decision that
    /// took nothing, or before the state is rebuilt.
    #[must_use]
    pub fn held_by(&self, decision_id: &str) -> Vec<(Scope, LimitKind, Holding)> {
        let Some(limits) = self.held.state() else { return Vec::new() };
        limits.held_by_decision.get(decision_id).map_or_else(Vec::new, |held| {
            held.iter().map(|((scope, kind), holding)| (scope.clone(), *kind, *holding)).collect()
        })
    }

    pub fn declare(
        &mut self,
        writer: &mut StreamWriter,
        scope: Scope,
        kind: LimitKind,
        maximum: Threshold,
        config_version: u64,
    ) -> Result<u64, StateError> {
        self.record(writer, LimitEvent::Declared { scope, kind, maximum, config_version })
    }
}
