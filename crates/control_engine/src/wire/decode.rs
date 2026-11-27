// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Bytes to control event. An unknown discriminant refuses rather than skips.

use super::*;

/// Decode a payload. An unknown class or a newer version is refused, never skipped.
pub fn decode(payload: &[u8]) -> Result<ControlEvent, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }

    // Refused before any field is read so the error names the real fault.
    if !matches!(
        envelope.class,
        ClassId::OrderIntent
            | ClassId::OrderSettled
            | ClassId::OrderWithdrawn
            | ClassId::VenueParticipation
            | ClassId::OrderRefused
            | ClassId::SafeStateDriven
            | ClassId::SafeStatePostcondition
            | ClassId::CommandApplied
            | ClassId::DecisionCaptured
            | ClassId::OrderFilled
            | ClassId::OrderReconciled
            | ClassId::ReconciliationCompleted
            | ClassId::PositionAdjusted
            | ClassId::DayOpened
            | ClassId::PortfolioValued
            | ClassId::DayPeak
            | ClassId::ScopesValued
            | ClassId::AccountCharged
            | ClassId::OrderHeld
            | ClassId::InstrumentScales
    ) {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let event = match envelope.class {
        ClassId::OrderIntent => decode_order_intent(&mut r)?,
        ClassId::OrderSettled => decode_order_settled(&mut r)?,
        ClassId::OrderFilled => {
            let client_id = r.str()?.to_string();
            let fill = read_fill(&mut r)?;
            ControlEvent::Filled { client_id, fill, at_ns: r.i128()? }
        }
        ClassId::OrderReconciled => decode_order_reconciled(&mut r)?,
        ClassId::PositionAdjusted => decode_position_adjusted(&mut r)?,
        ClassId::PortfolioValued => decode_portfolio_valued(&mut r)?,
        ClassId::ScopesValued => decode_scopes_valued(&mut r)?,
        ClassId::DayOpened => ControlEvent::DayOpened {
            day: r.u64()?,
            unrealized_scaled: r.i128()?,
            at_ns: r.i128()?,
        },
        ClassId::DayPeak => {
            ControlEvent::DayPeak { day: r.u64()?, result_scaled: r.i128()?, at_ns: r.i128()? }
        }
        ClassId::AccountCharged => decode_account_charged(&mut r)?,
        ClassId::OrderHeld => decode_order_held(&mut r)?,
        ClassId::InstrumentScales => decode_instrument_scales(&mut r)?,
        ClassId::ReconciliationCompleted => decode_reconciliation_completed(&mut r)?,
        ClassId::VenueParticipation => decode_venue_participation(&mut r)?,
        ClassId::OrderWithdrawn => decode_order_withdrawn(&mut r)?,
        ClassId::OrderRefused => decode_order_refused(&mut r)?,
        ClassId::DecisionCaptured => decode_decision_captured(&mut r)?,
        ClassId::CommandApplied => decode_command_applied(&mut r)?,
        ClassId::SafeStateDriven => decode_safe_state_driven(&mut r)?,
        ClassId::SafeStatePostcondition => decode_safe_state_postcondition(&mut r)?,
        // Catch-all by design, as in the limits decoder.
        _ => {
            return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
        }
    };

    r.finish()?;
    Ok(event)
}

fn decode_order_held(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let from_position = r.u64()?;
    let proposal = bijective_serialization::signal::decode(r.bytes()?)?;
    Ok(ControlEvent::Held { proposal, from_position, at_ns: r.i128()? })
}

/// A presence flag and, when set, the text.
fn read_text(r: &mut Reader<'_>, field: &'static str) -> Result<Option<String>, DecodeError> {
    Ok(if read_flag(r, field)? { Some(r.str()?.to_string()) } else { None })
}

fn decode_instrument_scales(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let venue = r.str()?.to_string();
    let instrument = VenueSymbol::new(r.str()?);
    let (price_scale, quantity_scale, at_ns) = (r.u8()?, r.u8()?, r.i128()?);
    // End of payload is where version 16 stops: scales recorded before the identity was.
    let (identity, quote_currency) = if r.more() {
        (read_text(r, "scales.identity")?, read_text(r, "scales.quote")?)
    } else {
        (None, None)
    };
    Ok(ControlEvent::InstrumentScales {
        venue,
        instrument,
        price_scale,
        quantity_scale,
        at_ns,
        identity,
        quote_currency,
    })
}

fn decode_account_charged(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let venue = r.str()?.to_string();
    let charge_id = r.str()?.to_string();
    let asset = r.str()?.to_string();
    let fee = bijective_lifecycle::Fee { asset, amount_scaled: r.i128()?, scale: r.u8()? };
    let charge = bijective_lifecycle::AccountCharge { charge_id, fee };
    Ok(ControlEvent::AccountCharged { venue, charge, at_ns: r.i128()? })
}

fn decode_order_reconciled(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let venue = r.str()?.to_string();
    let client_id = r.str()?.to_string();
    let settlement = read_settlement(r)?;
    Ok(ControlEvent::Reconciled { venue, client_id, settlement, at_ns: r.i128()? })
}

fn decode_reconciliation_completed(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let venue = r.str()?.to_string();
    let verdict = read_verdict(r)?;
    let findings = read_findings(r)?;
    let at_ns = r.i128()?;
    let evidence = read_evidence(r)?;
    Ok(ControlEvent::ReconciliationCompleted { venue, verdict, findings, evidence, at_ns })
}

fn decode_order_intent(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let from_position = r.u64()?;
    let decision_id = r.str()?.to_string();
    let client_id = r.str()?.to_string();
    let instrument = VenueSymbol::new(r.str()?);
    let side = match r.u8()? {
        SIDE_BUY => Side::Buy,
        SIDE_SELL => Side::Sell,
        other => {
            return Err(DecodeError::InvalidValue { field: "side", found: other as u64 });
        }
    };
    let quantity_scaled = r.i128()?;
    let price_scaled = if read_flag(r, "price_present")? { Some(r.i128()?) } else { None };
    let reduce_only = read_flag(r, "reduce_only")?;
    let at_ns = r.i128()?;
    // End of payload is where version 3 stops: an intent recorded before the venue was.
    let venue =
        if r.more() && read_flag(r, "intent.venue")? { Some(r.str()?.to_string()) } else { None };
    // End of payload is where version 4 stops: an intent recorded before its owner was.
    let owner = if r.more() && read_flag(r, "intent.owner")? {
        let desk = r.str()?.to_string();
        let strategy =
            if read_flag(r, "intent.strategy")? { Some(r.str()?.to_string()) } else { None };
        Some(Owner { desk, strategy })
    } else {
        None
    };
    // End of payload is where version 9 stops: an intent recorded before terms were (decision 0009).
    let terms = if r.more() {
        let code = r.u8()?;
        let time_in_force =
            bijective_model::TimeInForce::from_code(code).ok_or(DecodeError::InvalidValue {
                field: "intent.time_in_force",
                found: u64::from(code),
            })?;
        let post_only = read_flag(r, "intent.post_only")?;
        let stop_price_scaled =
            bijective_serialization::signal::proposal::read_stop(r, "intent.stop_price")?;
        // End of payload is where version 12 stops: an intent recorded before expiry (0014).
        let expire_at_ns =
            bijective_serialization::signal::proposal::read_stop(r, "intent.expire_at")?;
        bijective_model::OrderTerms { time_in_force, post_only, stop_price_scaled, expire_at_ns }
    } else {
        bijective_model::OrderTerms::default()
    };
    Ok(ControlEvent::Intent {
        decision_id,
        venue,
        owner,
        from_position,
        request: OrderRequest {
            client_id,
            instrument,
            side,
            quantity_scaled,
            price_scaled,
            reduce_only,
            terms,
        },
        at_ns,
    })
}

fn decode_order_settled(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let client_id = r.str()?.to_string();
    let outcome = read_outcome(r)?;
    let at_ns = r.i128()?;
    Ok(ControlEvent::Settled { client_id, outcome, at_ns })
}

fn decode_venue_participation(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let venue = r.str()?.to_string();
    let tag = r.u8()?;
    let reason = r.str()?.to_string();
    let participation = match tag {
        PARTICIPATION_JOINED => Participation::Participating,
        PARTICIPATION_WITHDRAWN => Participation::Withdrawn { reason },
        found => {
            return Err(DecodeError::InvalidValue {
                field: "participation",
                found: u64::from(found),
            });
        }
    };
    let at_ns = r.i128()?;
    Ok(ControlEvent::VenueParticipation { venue, participation, at_ns })
}

/// A count longer than the payload ends in a decode error at the first missing field.
fn decode_scopes_valued(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let count = r.u64()?;
    let mut figures = Vec::new();
    for _ in 0..count {
        let scope = read_scope(r)?;
        let kind = kind_from(r.u8()?)?;
        figures.push(ScopeFigure { scope, kind, value_scaled: r.i128()? });
    }
    Ok(ControlEvent::ScopesValued { figures, at_ns: r.i128()? })
}

fn read_figure(r: &mut Reader<'_>) -> Result<Option<i128>, DecodeError> {
    let present = match r.u8()? {
        0 => false,
        1 => true,
        other => {
            return Err(DecodeError::InvalidValue { field: "figure", found: u64::from(other) });
        }
    };
    let value = r.i128()?;
    Ok(present.then_some(value))
}

fn decode_portfolio_valued(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let (gross_scaled, net_scaled) = (read_figure(r)?, read_figure(r)?);
    let daily_loss_scaled = read_figure(r)?;
    let observed_at_ns = r.i128()?;
    let at_ns = r.i128()?;
    // End of payload is where version 13 stops: a summary recorded before ratios were (0011).
    let (leverage_scaled, margin_use_scaled) =
        if r.more() { (read_figure(r)?, read_figure(r)?) } else { (None, None) };
    // And where version 15 stops: a summary recorded before the equity was.
    let equity_scaled = if r.more() { read_figure(r)? } else { None };
    let figures = Valued {
        gross_scaled,
        net_scaled,
        daily_loss_scaled,
        observed_at_ns,
        leverage_scaled,
        margin_use_scaled,
        equity_scaled,
    };
    Ok(ControlEvent::PortfolioValued { figures, at_ns })
}

fn decode_order_withdrawn(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let client_id = r.str()?.to_string();
    let instrument = VenueSymbol::new(r.str()?);
    let outcome = read_outcome(r)?;
    let at_ns = r.i128()?;
    Ok(ControlEvent::Withdrawn { client_id, instrument, outcome, at_ns })
}

fn decode_order_refused(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let from_position = r.u64()?;
    let decision_id = r.str()?.to_string();
    let reason = match r.u8()? {
        REFUSED_NOT_ACTIVE => RefusalReason::NotActive { state: state_from(r.u8()?)? },
        REFUSED_UNRECOVERED => RefusalReason::LimitsUnrecovered,
        REFUSED_BREACHED => RefusalReason::Breached,
        REFUSED_UNJUDGED => RefusalReason::Unjudged { kind: kind_from(r.u8()?)? },
        REFUSED_STALE => RefusalReason::Stale { age_ns: r.i128()? },
        REFUSED_SKEW => RefusalReason::ClockSkew { skew_ns: r.i128()? },
        REFUSED_NO_BUDGET => {
            RefusalReason::NoBudget { scope: read_scope(r)?, kind: kind_from(r.u8()?)? }
        }
        REFUSED_NOT_ADMITTED => RefusalReason::NotAdmitted { detail: r.str()?.to_string() },
        REFUSED_UNREADABLE => RefusalReason::Unreadable,
        REFUSED_DOES_NOT_REDUCE => {
            RefusalReason::DoesNotReduce { held_scaled: r.i128()?, working_scaled: r.i128()? }
        }
        REFUSED_RATE_LIMITED => RefusalReason::RateLimited { retry_after_ns: r.i128()? },
        REFUSED_UNCOVERED => {
            RefusalReason::Uncovered { held_scaled: r.i128()?, working_scaled: r.i128()? }
        }
        REFUSED_INSTRUMENT => {
            let symbol = VenueSymbol::new(r.str()?);
            let cause = match r.u8()? {
                INSTRUMENT_UNKNOWN => InstrumentRefusal::Unknown,
                INSTRUMENT_HALTED => InstrumentRefusal::Halted,
                INSTRUMENT_DELISTED => InstrumentRefusal::Delisted,
                INSTRUMENT_BELOW_MINIMUM => InstrumentRefusal::BelowMinimumSize,
                other => {
                    return Err(DecodeError::InvalidValue {
                        field: "instrument_refusal",
                        found: other as u64,
                    });
                }
            };
            RefusalReason::InstrumentRefused { symbol, cause }
        }
        other => {
            return Err(DecodeError::InvalidValue { field: "refusal_reason", found: other as u64 });
        }
    };
    let at_ns = r.i128()?;
    Ok(ControlEvent::Refused { decision_id, reason, from_position, at_ns })
}

fn decode_decision_captured(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let from_position = r.u64()?;
    let correlation_id = r.str()?.to_string();
    let instrument = VenueSymbol::new(r.str()?);
    let decided_at_ns = r.i128()?;
    let book = if read_flag(r, "book_present")? {
        Some(CapturedBook {
            best_bid_ticks: r.i64()?,
            best_bid_qty_lots: r.i64()?,
            best_ask_ticks: r.i64()?,
            best_ask_qty_lots: r.i64()?,
            observed_at_ns: r.i128()?,
            capture_lag_ns: r.i128()?,
        })
    } else {
        None
    };
    let detail = r.str()?.to_string();
    let at_ns = r.i128()?;
    Ok(ControlEvent::Captured {
        correlation_id,
        instrument,
        decided_at_ns,
        book,
        detail,
        from_position,
        at_ns,
    })
}

fn decode_command_applied(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let from_position = r.u64()?;
    let command_id = r.str()?.to_string();
    let actor = r.str()?.to_string();
    let role_text = r.str()?;
    let role = Role::named(role_text)
        .ok_or(DecodeError::InvalidValue { field: "role", found: role_text.len() as u64 })?;
    let action_text = r.str()?;
    let action = Action::named(action_text)
        .ok_or(DecodeError::InvalidValue { field: "action", found: action_text.len() as u64 })?;
    let state = state_from(r.u8()?)?;
    let detail = r.str()?.to_string();
    let at_ns = r.i128()?;
    Ok(ControlEvent::CommandApplied {
        command_id,
        actor,
        role,
        action,
        state,
        detail,
        from_position,
        at_ns,
    })
}

fn decode_safe_state_driven(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let target = state_from(r.u8()?)?;
    let reason = read_reason(r)?;
    let arrived = read_flag(r, "arrived")?;
    let at_ns = r.i128()?;
    // End of payload is where version 1 stops: a truncation, not a fault.
    let scope =
        if r.more() && read_flag(r, "safe-state.scope")? { Some(read_scope(r)?) } else { None };
    Ok(ControlEvent::SafeStateDriven { target, reason, arrived, at_ns, scope })
}

fn decode_safe_state_postcondition(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let scope = read_scope(r)?;
    let kind = kind_from(r.u8()?)?;
    let before = read_threshold(r)?;
    let at_ns = r.i128()?;
    let outcome = match r.u8()? {
        POSTCONDITION_HELD => PostconditionOutcome::Held { after: read_threshold(r)? },
        POSTCONDITION_INCREASED => PostconditionOutcome::Increased { after: read_threshold(r)? },
        POSTCONDITION_UNVERIFIABLE => PostconditionOutcome::Unverifiable,
        other => {
            return Err(DecodeError::InvalidValue {
                field: "postcondition.outcome",
                found: other as u64,
            });
        }
    };
    Ok(ControlEvent::SafeStatePostcondition { scope, kind, before, outcome, at_ns })
}
