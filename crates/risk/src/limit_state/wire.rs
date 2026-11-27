// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The three limit-state class layouts. Discriminants are pinned constants, never
//! declaration order: a reordered enum would read a desk limit back as a venue limit.

use bijective_limits::{Amount, LimitKind, Ratio, Scope, Threshold};
use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

/// The layout version this build writes and is the newest it reads. Version 2 appends the
/// decision a release returns budget for. A version-1 release names none. Version 3 adds the
/// underlying scope level, a new discriminant.
pub const VERSION: u16 = 3;

/// Why budget came back. Named rather than signed, because an auditor needs the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    PeriodRollover,
    OrderCancelled,
    PositionClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitEvent {
    Declared {
        scope: Scope,
        kind: LimitKind,
        maximum: Threshold,
        config_version: u64,
    },
    Consumed {
        scope: Scope,
        kind: LimitKind,
        amount: Amount,
        decision_id: String,
    },
    Released {
        scope: Scope,
        kind: LimitKind,
        amount: Amount,
        reason: ReleaseReason,
        /// The decision whose reservation this returns. `None` for budget not held by one order,
        /// such as a period rollover, and for every version-1 record.
        decision_id: Option<String>,
    },
}

// pinned wire discriminants

const DECISION_NONE: u8 = 0;
const DECISION_NAMED: u8 = 1;

const LEVEL_PORTFOLIO: u8 = 0;
const LEVEL_DESK: u8 = 1;
const LEVEL_VENUE: u8 = 2;
const LEVEL_STRATEGY: u8 = 3;
const LEVEL_INSTRUMENT: u8 = 4;
const LEVEL_UNDERLYING: u8 = 5;

const KIND_GROSS: u8 = 0;
const KIND_NET: u8 = 1;
const KIND_CONCENTRATION: u8 = 2;
const KIND_LEVERAGE: u8 = 3;
const KIND_DAILY_LOSS: u8 = 4;
const KIND_DRAWDOWN: u8 = 5;
const KIND_MARGIN_USE: u8 = 6;

const THRESHOLD_MONEY: u8 = 0;
const THRESHOLD_RATIO: u8 = 1;

const RELEASE_PERIOD: u8 = 0;
const RELEASE_CANCELLED: u8 = 1;
const RELEASE_CLOSED: u8 = 2;

const fn kind_code(kind: LimitKind) -> u8 {
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

fn kind_from(code: u8) -> Result<LimitKind, DecodeError> {
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

/// A scope is a level plus a name. The portfolio level carries an empty name.
fn write_scope(w: &mut Writer, scope: &Scope) -> Result<(), EncodeError> {
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

fn read_scope(r: &mut Reader<'_>) -> Result<Scope, DecodeError> {
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

pub fn encode(event: &LimitEvent) -> Result<Vec<u8>, EncodeError> {
    match event {
        LimitEvent::Declared { scope, kind, maximum, config_version } => {
            let mut w = Writer::new(ClassId::LimitDeclared, VERSION);
            write_scope(&mut w, scope)?;
            w.u8(kind_code(*kind));
            match maximum {
                Threshold::Money(a) => {
                    w.u8(THRESHOLD_MONEY).i128(a.scaled());
                }
                Threshold::Ratio(r) => {
                    w.u8(THRESHOLD_RATIO).i128(r.scaled());
                }
            }
            w.u64(*config_version);
            Ok(w.finish())
        }
        LimitEvent::Consumed { scope, kind, amount, decision_id } => {
            let mut w = Writer::new(ClassId::LimitConsumed, VERSION);
            write_scope(&mut w, scope)?;
            w.u8(kind_code(*kind)).i128(amount.scaled());
            w.str(decision_id)?;
            Ok(w.finish())
        }
        LimitEvent::Released { scope, kind, amount, reason, decision_id } => {
            let mut w = Writer::new(ClassId::LimitReleased, VERSION);
            write_scope(&mut w, scope)?;
            w.u8(kind_code(*kind)).i128(amount.scaled());
            w.u8(match reason {
                ReleaseReason::PeriodRollover => RELEASE_PERIOD,
                ReleaseReason::OrderCancelled => RELEASE_CANCELLED,
                ReleaseReason::PositionClosed => RELEASE_CLOSED,
            });
            match decision_id {
                Some(id) => {
                    w.u8(DECISION_NAMED).str(id)?;
                }
                None => {
                    w.u8(DECISION_NONE);
                }
            }
            Ok(w.finish())
        }
    }
}

/// Decode a payload. An unknown class or a newer version is refused, never skipped - skipping
/// one *consumed* event restores budget silently.
pub fn decode(payload: &[u8]) -> Result<LimitEvent, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }

    // Refuse a foreign class before any field is read, else the error would describe a bad scope.
    if !matches!(
        envelope.class,
        ClassId::LimitDeclared | ClassId::LimitConsumed | ClassId::LimitReleased
    ) {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let scope = read_scope(&mut r)?;
    let kind = kind_from(r.u8()?)?;

    let event = match envelope.class {
        ClassId::LimitDeclared => {
            let tag = r.u8()?;
            let value = r.i128()?;
            let maximum = match tag {
                THRESHOLD_MONEY => Threshold::Money(Amount::from_scaled(value)),
                THRESHOLD_RATIO => Threshold::Ratio(Ratio::from_scaled(value)),
                other => {
                    return Err(DecodeError::InvalidValue {
                        field: "threshold_tag",
                        found: other as u64,
                    });
                }
            };
            LimitEvent::Declared { scope, kind, maximum, config_version: r.u64()? }
        }
        ClassId::LimitConsumed => LimitEvent::Consumed {
            scope,
            kind,
            amount: Amount::from_scaled(r.i128()?),
            decision_id: r.str()?.to_string(),
        },
        ClassId::LimitReleased => decode_released(&mut r, scope, kind)?,
        // Catch-all, not `unreachable!()`: a class added to the registry must fail closed, not panic.
        _ => {
            return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
        }
    };

    r.finish()?;
    Ok(event)
}

fn decode_released(
    r: &mut Reader<'_>,
    scope: Scope,
    kind: LimitKind,
) -> Result<LimitEvent, DecodeError> {
    let amount = Amount::from_scaled(r.i128()?);
    let reason = match r.u8()? {
        RELEASE_PERIOD => ReleaseReason::PeriodRollover,
        RELEASE_CANCELLED => ReleaseReason::OrderCancelled,
        RELEASE_CLOSED => ReleaseReason::PositionClosed,
        other => {
            return Err(DecodeError::InvalidValue { field: "release_reason", found: other as u64 });
        }
    };
    // Appended at version 2: a version-1 release ends here and names no decision.
    let decision_id = if r.more() {
        match r.u8()? {
            DECISION_NONE => None,
            DECISION_NAMED => Some(r.str()?.to_string()),
            other => {
                return Err(DecodeError::InvalidValue {
                    field: "release_decision",
                    found: u64::from(other),
                });
            }
        }
    } else {
        None
    };
    Ok(LimitEvent::Released { scope, kind, amount, reason, decision_id })
}
