// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Wire discriminants and the field encoders shared by every control event. Values are fixed: a renumbered code reinterprets stored history.

use super::*;
use bijective_venue::normalise::{Rejection, Unknown};

pub(crate) const LEVEL_PORTFOLIO: u8 = 0;

pub(crate) const LEVEL_DESK: u8 = 1;

pub(crate) const LEVEL_VENUE: u8 = 2;

pub(crate) const LEVEL_STRATEGY: u8 = 3;

pub(crate) const LEVEL_INSTRUMENT: u8 = 4;

pub(crate) const LEVEL_UNDERLYING: u8 = 5;

pub(crate) const KIND_GROSS: u8 = 0;

pub(crate) const KIND_NET: u8 = 1;

pub(crate) const KIND_CONCENTRATION: u8 = 2;

pub(crate) const KIND_LEVERAGE: u8 = 3;

pub(crate) const KIND_DAILY_LOSS: u8 = 4;

pub(crate) const KIND_DRAWDOWN: u8 = 5;
pub(crate) const KIND_MARGIN_USE: u8 = 6;

pub(crate) const THRESHOLD_MONEY: u8 = 0;

pub(crate) const THRESHOLD_RATIO: u8 = 1;

pub(crate) const SIDE_BUY: u8 = 0;

pub(crate) const SIDE_SELL: u8 = 1;

pub(crate) const PARTICIPATION_JOINED: u8 = 0;

pub(crate) const PARTICIPATION_WITHDRAWN: u8 = 1;

pub(crate) const STATE_ACTIVE: u8 = 0;

pub(crate) const STATE_REDUCING: u8 = 1;

pub(crate) const STATE_HALTED: u8 = 2;

pub(crate) const REASON_NEVER_PRIMED: u8 = 0;

pub(crate) const REASON_INPUTS_STALE: u8 = 1;

pub(crate) const REASON_UNJUDGEABLE: u8 = 2;

pub(crate) const REASON_BREACHED: u8 = 3;

pub(crate) const REASON_DIVERGED: u8 = 4;

pub(crate) const REASON_UNREAD: u8 = 5;

pub(crate) const REFUSED_NOT_ACTIVE: u8 = 0;

pub(crate) const REFUSED_UNRECOVERED: u8 = 1;

pub(crate) const REFUSED_BREACHED: u8 = 2;

pub(crate) const REFUSED_UNJUDGED: u8 = 3;

pub(crate) const REFUSED_STALE: u8 = 4;

pub(crate) const REFUSED_SKEW: u8 = 5;

pub(crate) const REFUSED_NO_BUDGET: u8 = 6;

pub(crate) const REFUSED_INSTRUMENT: u8 = 7;

pub(crate) const REFUSED_NOT_ADMITTED: u8 = 8;

pub(crate) const REFUSED_UNREADABLE: u8 = 9;

pub(crate) const REFUSED_DOES_NOT_REDUCE: u8 = 10;

pub(crate) const REFUSED_RATE_LIMITED: u8 = 11;

pub(crate) const REFUSED_UNCOVERED: u8 = 12;

pub(crate) const INSTRUMENT_UNKNOWN: u8 = 0;

pub(crate) const INSTRUMENT_HALTED: u8 = 1;

pub(crate) const INSTRUMENT_DELISTED: u8 = 2;

pub(crate) const INSTRUMENT_BELOW_MINIMUM: u8 = 3;

pub(crate) const OUTCOME_ACCEPTED: u8 = 0;

pub(crate) const OUTCOME_REJECTED: u8 = 1;

pub(crate) const OUTCOME_UNKNOWN: u8 = 2;

pub(crate) const OUTCOME_NOT_SENT: u8 = 3;

pub(crate) const REJECT_RATE_LIMITED: u8 = 0;

pub(crate) const REJECT_MARGIN: u8 = 1;

pub(crate) const REJECT_REDUCE_ONLY: u8 = 2;

pub(crate) const REJECT_INSTRUMENT: u8 = 3;

pub(crate) const REJECT_PRICE_BAND: u8 = 4;

pub(crate) const REJECT_MIN_SIZE: u8 = 5;

pub(crate) const REJECT_IDENTIFIER: u8 = 6;

pub(crate) const REJECT_CREDENTIAL: u8 = 7;

pub(crate) const REJECT_DUPLICATE: u8 = 8;

pub(crate) const REJECT_UNCLASSIFIED: u8 = 9;

pub(crate) const UNKNOWN_TIMEOUT: u8 = 0;

pub(crate) const UNKNOWN_CONNECTION_LOST: u8 = 1;

pub(crate) const UNKNOWN_UNREADABLE: u8 = 2;

pub(crate) const ABSENT: u8 = 0;

pub(crate) const PRESENT: u8 = 1;

pub(crate) const POSTCONDITION_HELD: u8 = 0;

pub(crate) const POSTCONDITION_INCREASED: u8 = 1;

pub(crate) const POSTCONDITION_UNVERIFIABLE: u8 = 2;

pub(crate) const fn kind_code(kind: LimitKind) -> u8 {
    match kind {
        LimitKind::GrossExposure => KIND_GROSS,
        LimitKind::NetExposure => KIND_NET,
        LimitKind::Concentration => KIND_CONCENTRATION,
        LimitKind::Leverage => KIND_LEVERAGE,
        LimitKind::DailyLoss => KIND_DAILY_LOSS,
        LimitKind::Drawdown => KIND_DRAWDOWN,
        LimitKind::MarginUse => KIND_MARGIN_USE,
    }
}

pub(crate) fn kind_from(code: u8) -> Result<LimitKind, DecodeError> {
    match code {
        KIND_GROSS => Ok(LimitKind::GrossExposure),
        KIND_NET => Ok(LimitKind::NetExposure),
        KIND_CONCENTRATION => Ok(LimitKind::Concentration),
        KIND_LEVERAGE => Ok(LimitKind::Leverage),
        KIND_DAILY_LOSS => Ok(LimitKind::DailyLoss),
        KIND_DRAWDOWN => Ok(LimitKind::Drawdown),
        KIND_MARGIN_USE => Ok(LimitKind::MarginUse),
        other => Err(DecodeError::InvalidValue { field: "kind", found: other as u64 }),
    }
}

pub(crate) const fn level_code(level: ScopeLevel) -> u8 {
    match level {
        ScopeLevel::Portfolio => LEVEL_PORTFOLIO,
        ScopeLevel::Desk => LEVEL_DESK,
        ScopeLevel::Venue => LEVEL_VENUE,
        ScopeLevel::Strategy => LEVEL_STRATEGY,
        ScopeLevel::Instrument => LEVEL_INSTRUMENT,
        ScopeLevel::Underlying => LEVEL_UNDERLYING,
    }
}

pub(crate) fn level_from(code: u8) -> Result<ScopeLevel, DecodeError> {
    match code {
        LEVEL_PORTFOLIO => Ok(ScopeLevel::Portfolio),
        LEVEL_DESK => Ok(ScopeLevel::Desk),
        LEVEL_VENUE => Ok(ScopeLevel::Venue),
        LEVEL_STRATEGY => Ok(ScopeLevel::Strategy),
        LEVEL_UNDERLYING => Ok(ScopeLevel::Underlying),
        LEVEL_INSTRUMENT => Ok(ScopeLevel::Instrument),
        other => Err(DecodeError::InvalidValue { field: "scope_level", found: other as u64 }),
    }
}

pub(crate) const fn state_code(state: SafeState) -> u8 {
    match state {
        SafeState::Active => STATE_ACTIVE,
        SafeState::Reducing => STATE_REDUCING,
        SafeState::Halted => STATE_HALTED,
    }
}

pub(crate) fn state_from(code: u8) -> Result<SafeState, DecodeError> {
    match code {
        STATE_ACTIVE => Ok(SafeState::Active),
        STATE_REDUCING => Ok(SafeState::Reducing),
        STATE_HALTED => Ok(SafeState::Halted),
        other => Err(DecodeError::InvalidValue { field: "safe_state", found: other as u64 }),
    }
}

/// A threshold is a tag plus its scaled value. The tag carries the unit.
pub(crate) fn write_threshold(w: &mut Writer, threshold: Threshold) {
    match threshold {
        Threshold::Money(a) => {
            w.u8(THRESHOLD_MONEY).i128(a.scaled());
        }
        Threshold::Ratio(r) => {
            w.u8(THRESHOLD_RATIO).i128(r.scaled());
        }
    }
}

pub(crate) fn read_threshold(r: &mut Reader<'_>) -> Result<Threshold, DecodeError> {
    let tag = r.u8()?;
    let value = r.i128()?;
    match tag {
        THRESHOLD_MONEY => Ok(Threshold::Money(Amount::from_scaled(value))),
        THRESHOLD_RATIO => Ok(Threshold::Ratio(Ratio::from_scaled(value))),
        other => Err(DecodeError::InvalidValue { field: "threshold_tag", found: other as u64 }),
    }
}

/// A scope is a level plus a name. The portfolio level carries an empty name.
pub(crate) fn write_scope(w: &mut Writer, scope: &Scope) -> Result<(), EncodeError> {
    let (code, name) = match scope {
        Scope::Portfolio => (LEVEL_PORTFOLIO, ""),
        Scope::Desk(n) => (LEVEL_DESK, n.as_str()),
        Scope::Venue(n) => (LEVEL_VENUE, n.as_str()),
        Scope::Strategy(n) => (LEVEL_STRATEGY, n.as_str()),
        Scope::Instrument(n) => (LEVEL_INSTRUMENT, n.as_str()),
        Scope::Underlying(n) => (LEVEL_UNDERLYING, n.as_str()),
    };
    w.u8(code);
    w.str(name)?;
    Ok(())
}

pub(crate) fn read_scope(r: &mut Reader<'_>) -> Result<Scope, DecodeError> {
    let code = r.u8()?;
    let name = r.str()?.to_string();
    match code {
        LEVEL_PORTFOLIO => Ok(Scope::Portfolio),
        LEVEL_DESK => Ok(Scope::Desk(name)),
        LEVEL_VENUE => Ok(Scope::Venue(name)),
        LEVEL_STRATEGY => Ok(Scope::Strategy(name)),
        LEVEL_INSTRUMENT => Ok(Scope::Instrument(name)),
        LEVEL_UNDERLYING => Ok(Scope::Underlying(name)),
        other => Err(DecodeError::InvalidValue { field: "scope_level", found: other as u64 }),
    }
}

pub(crate) fn write_reason(w: &mut Writer, reason: SafeStateReason) {
    match reason {
        SafeStateReason::NeverPrimed => {
            w.u8(REASON_NEVER_PRIMED);
        }
        SafeStateReason::InputsStale { stale_for_ns } => {
            w.u8(REASON_INPUTS_STALE).i128(stale_for_ns);
        }
        SafeStateReason::Unjudgeable { kind } => {
            w.u8(REASON_UNJUDGEABLE).u8(kind_code(kind));
        }
        SafeStateReason::Breached(breach) => {
            w.u8(REASON_BREACHED).u8(level_code(breach.level)).u8(kind_code(breach.kind));
            write_threshold(w, breach.observed);
            write_threshold(w, breach.maximum);
        }
        SafeStateReason::ReconciliationDiverged => {
            w.u8(REASON_DIVERGED);
        }
        SafeStateReason::EvidenceUnread => {
            w.u8(REASON_UNREAD);
        }
    }
}

pub(crate) fn read_reason(r: &mut Reader<'_>) -> Result<SafeStateReason, DecodeError> {
    match r.u8()? {
        REASON_NEVER_PRIMED => Ok(SafeStateReason::NeverPrimed),
        REASON_INPUTS_STALE => Ok(SafeStateReason::InputsStale { stale_for_ns: r.i128()? }),
        REASON_UNJUDGEABLE => Ok(SafeStateReason::Unjudgeable { kind: kind_from(r.u8()?)? }),
        REASON_BREACHED => {
            let level = level_from(r.u8()?)?;
            let kind = kind_from(r.u8()?)?;
            let observed = read_threshold(r)?;
            let maximum = read_threshold(r)?;
            Ok(SafeStateReason::Breached(Breach { level, kind, observed, maximum }))
        }
        REASON_DIVERGED => Ok(SafeStateReason::ReconciliationDiverged),
        REASON_UNREAD => Ok(SafeStateReason::EvidenceUnread),
        other => Err(DecodeError::InvalidValue { field: "safe_state_reason", found: other as u64 }),
    }
}

pub(crate) fn write_outcome(w: &mut Writer, outcome: &VenueOutcome) -> Result<(), EncodeError> {
    match outcome {
        VenueOutcome::Accepted => {
            w.u8(OUTCOME_ACCEPTED);
        }
        VenueOutcome::NotSent => {
            w.u8(OUTCOME_NOT_SENT);
        }
        VenueOutcome::Rejected(rejection) => {
            w.u8(OUTCOME_REJECTED);
            match rejection {
                Rejection::RateLimited { retry_after_ns } => {
                    w.u8(REJECT_RATE_LIMITED);
                    match retry_after_ns {
                        Some(ns) => {
                            w.u8(PRESENT).i128(*ns);
                        }
                        None => {
                            w.u8(ABSENT);
                        }
                    }
                }
                Rejection::InsufficientMargin => {
                    w.u8(REJECT_MARGIN);
                }
                Rejection::ReduceOnlyViolated => {
                    w.u8(REJECT_REDUCE_ONLY);
                }
                Rejection::UnknownInstrument => {
                    w.u8(REJECT_INSTRUMENT);
                }
                Rejection::PriceOutOfBand => {
                    w.u8(REJECT_PRICE_BAND);
                }
                Rejection::BelowMinimumSize => {
                    w.u8(REJECT_MIN_SIZE);
                }
                Rejection::IdentifierRejected => {
                    w.u8(REJECT_IDENTIFIER);
                }
                Rejection::CredentialRefused => {
                    w.u8(REJECT_CREDENTIAL);
                }
                Rejection::Duplicate => {
                    w.u8(REJECT_DUPLICATE);
                }
                // Verbatim: mapping it to a known rejection would erase evidence that a venue changed.
                Rejection::Unclassified { venue_code } => {
                    w.u8(REJECT_UNCLASSIFIED);
                    w.str(venue_code)?;
                }
            }
        }
        VenueOutcome::Unknown(unknown) => {
            w.u8(OUTCOME_UNKNOWN).u8(match unknown {
                Unknown::Timeout => UNKNOWN_TIMEOUT,
                Unknown::ConnectionLost => UNKNOWN_CONNECTION_LOST,
                Unknown::Unreadable => UNKNOWN_UNREADABLE,
            });
        }
    }
    Ok(())
}

pub(crate) fn read_outcome(r: &mut Reader<'_>) -> Result<VenueOutcome, DecodeError> {
    match r.u8()? {
        OUTCOME_ACCEPTED => Ok(VenueOutcome::Accepted),
        OUTCOME_NOT_SENT => Ok(VenueOutcome::NotSent),
        OUTCOME_REJECTED => {
            let rejection = match r.u8()? {
                REJECT_RATE_LIMITED => {
                    let retry_after_ns = match r.u8()? {
                        PRESENT => Some(r.i128()?),
                        ABSENT => None,
                        other => {
                            return Err(DecodeError::InvalidValue {
                                field: "retry_after_present",
                                found: other as u64,
                            });
                        }
                    };
                    Rejection::RateLimited { retry_after_ns }
                }
                REJECT_MARGIN => Rejection::InsufficientMargin,
                REJECT_REDUCE_ONLY => Rejection::ReduceOnlyViolated,
                REJECT_INSTRUMENT => Rejection::UnknownInstrument,
                REJECT_PRICE_BAND => Rejection::PriceOutOfBand,
                REJECT_MIN_SIZE => Rejection::BelowMinimumSize,
                REJECT_IDENTIFIER => Rejection::IdentifierRejected,
                REJECT_CREDENTIAL => Rejection::CredentialRefused,
                REJECT_DUPLICATE => Rejection::Duplicate,
                REJECT_UNCLASSIFIED => Rejection::Unclassified { venue_code: r.str()?.to_string() },
                other => {
                    return Err(DecodeError::InvalidValue {
                        field: "rejection",
                        found: other as u64,
                    });
                }
            };
            Ok(VenueOutcome::Rejected(rejection))
        }
        OUTCOME_UNKNOWN => match r.u8()? {
            UNKNOWN_TIMEOUT => Ok(VenueOutcome::Unknown(Unknown::Timeout)),
            UNKNOWN_CONNECTION_LOST => Ok(VenueOutcome::Unknown(Unknown::ConnectionLost)),
            UNKNOWN_UNREADABLE => Ok(VenueOutcome::Unknown(Unknown::Unreadable)),
            other => Err(DecodeError::InvalidValue { field: "unknown_kind", found: other as u64 }),
        },
        other => Err(DecodeError::InvalidValue { field: "outcome", found: other as u64 }),
    }
}

/// An action by the name it was written under, so a record reads without this build.
pub(crate) fn read_flag(r: &mut Reader<'_>, field: &'static str) -> Result<bool, DecodeError> {
    match r.u8()? {
        ABSENT => Ok(false),
        PRESENT => Ok(true),
        other => Err(DecodeError::InvalidValue { field, found: other as u64 }),
    }
}
