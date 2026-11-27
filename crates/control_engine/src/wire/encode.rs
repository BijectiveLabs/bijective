// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Control event to bytes.

use super::*;

/// Encode an event into a payload the store can take.
pub fn encode(event: &ControlEvent) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(class_of(event), VERSION);
    write_fields(&mut w, event)?;
    Ok(w.finish())
}

fn class_of(event: &ControlEvent) -> ClassId {
    match event {
        ControlEvent::Intent { .. } => ClassId::OrderIntent,
        ControlEvent::Settled { .. } => ClassId::OrderSettled,
        ControlEvent::Withdrawn { .. } => ClassId::OrderWithdrawn,
        ControlEvent::VenueParticipation { .. } => ClassId::VenueParticipation,
        ControlEvent::Refused { .. } => ClassId::OrderRefused,
        ControlEvent::Captured { .. } => ClassId::DecisionCaptured,
        ControlEvent::CommandApplied { .. } => ClassId::CommandApplied,
        ControlEvent::SafeStateDriven { .. } => ClassId::SafeStateDriven,
        ControlEvent::SafeStatePostcondition { .. } => ClassId::SafeStatePostcondition,
        ControlEvent::Filled { .. } => ClassId::OrderFilled,
        ControlEvent::Reconciled { .. } => ClassId::OrderReconciled,
        ControlEvent::ReconciliationCompleted { .. } => ClassId::ReconciliationCompleted,
        ControlEvent::PositionAdjusted { .. } => ClassId::PositionAdjusted,
        ControlEvent::DayOpened { .. } => ClassId::DayOpened,
        ControlEvent::PortfolioValued { .. } => ClassId::PortfolioValued,
        ControlEvent::ScopesValued { .. } => ClassId::ScopesValued,
        ControlEvent::AccountCharged { .. } => ClassId::AccountCharged,
        ControlEvent::DayPeak { .. } => ClassId::DayPeak,
        ControlEvent::Held { .. } => ClassId::OrderHeld,
        ControlEvent::InstrumentScales { .. } => ClassId::InstrumentScales,
    }
}

/// The venue follows, after a presence flag: appended at version 4, so earlier intents end
/// before it. The owner is appended after it at version 5, a flag and the desk, then a flag and
/// the strategy.
fn write_intent(w: &mut Writer, event: &ControlEvent) -> Result<(), EncodeError> {
    let ControlEvent::Intent { decision_id, venue, owner, request, from_position, at_ns } = event
    else {
        return Ok(());
    };
    w.u64(*from_position);
    w.str(decision_id)?;
    write_request(w, request)?;
    w.i128(*at_ns);
    write_present(w, venue.as_deref())?;
    w.u8(u8::from(owner.is_some()));
    if let Some(owner) = owner {
        w.str(&owner.desk)?;
        write_present(w, owner.strategy.as_deref())?;
    }
    w.u8(request.terms.time_in_force.code()).u8(u8::from(request.terms.post_only));
    bijective_serialization::signal::proposal::write_stop(w, request.terms.stop_price_scaled);
    bijective_serialization::signal::proposal::write_stop(w, request.terms.expire_at_ns);
    Ok(())
}

fn write_present(w: &mut Writer, text: Option<&str>) -> Result<(), EncodeError> {
    w.u8(u8::from(text.is_some()));
    if let Some(text) = text {
        w.str(text)?;
    }
    Ok(())
}

/// Field order is the wire format: a new field goes on the end, never inserted.
fn write_fields(w: &mut Writer, event: &ControlEvent) -> Result<(), EncodeError> {
    match event {
        ControlEvent::Intent { .. }
        | ControlEvent::Filled { .. }
        | ControlEvent::Reconciled { .. }
        | ControlEvent::ReconciliationCompleted { .. }
        | ControlEvent::Settled { .. }
        | ControlEvent::Withdrawn { .. }
        | ControlEvent::InstrumentScales { .. } => write_order_fields(w, event)?,
        ControlEvent::PositionAdjusted { .. }
        | ControlEvent::PortfolioValued { .. }
        | ControlEvent::DayOpened { .. }
        | ControlEvent::DayPeak { .. }
        | ControlEvent::ScopesValued { .. }
        | ControlEvent::AccountCharged { .. } => write_book_fields(w, event)?,
        ControlEvent::VenueParticipation { venue, participation, at_ns } => {
            w.str(venue)?;
            write_participation(w, participation)?;
            w.i128(*at_ns);
        }
        ControlEvent::Held { proposal, from_position, at_ns } => {
            w.u64(*from_position);
            w.bytes(&bijective_serialization::signal::encode(proposal)?)?;
            w.i128(*at_ns);
        }
        ControlEvent::Refused { decision_id, reason, from_position, at_ns } => {
            w.u64(*from_position);
            w.str(decision_id)?;
            write_refusal(w, reason)?;
            w.i128(*at_ns);
        }
        ControlEvent::Captured {
            correlation_id,
            instrument,
            decided_at_ns,
            book,
            detail,
            from_position,
            at_ns,
        } => {
            w.u64(*from_position);
            write_decision(w, correlation_id, instrument, *decided_at_ns, book.as_ref())?;
            w.str(detail)?;
            w.i128(*at_ns);
        }
        ControlEvent::CommandApplied {
            command_id,
            actor,
            role,
            action,
            state,
            detail,
            from_position,
            at_ns,
        } => {
            w.u64(*from_position);
            w.str(command_id)?;
            w.str(actor)?;
            write_authority(w, *role, *action, *state)?;
            w.str(detail)?;
            w.i128(*at_ns);
        }
        ControlEvent::SafeStateDriven { target, reason, arrived, at_ns, scope } => {
            w.u8(state_code(*target));
            write_reason(w, *reason);
            w.u8(u8::from(*arrived));
            w.i128(*at_ns);
            write_optional_scope(w, scope.as_ref())?;
        }
        ControlEvent::SafeStatePostcondition { scope, kind, before, outcome, at_ns } => {
            write_scope(w, scope)?;
            w.u8(kind_code(*kind));
            write_threshold(w, *before);
            w.i128(*at_ns);
            write_postcondition(w, outcome);
        }
    }
    Ok(())
}

/// The classes about the whole book, split from `write_fields` like the order classes.
fn write_book_fields(w: &mut Writer, event: &ControlEvent) -> Result<(), EncodeError> {
    match event {
        ControlEvent::PositionAdjusted { adjustment, at_ns } => {
            write_adjustment(w, adjustment, *at_ns)?;
        }
        ControlEvent::PortfolioValued { figures, at_ns } => {
            let write_figure = |w: &mut Writer, figure: Option<i128>| {
                w.u8(u8::from(figure.is_some()));
                w.i128(figure.unwrap_or(0));
            };
            for figure in [figures.gross_scaled, figures.net_scaled, figures.daily_loss_scaled] {
                write_figure(w, figure);
            }
            w.i128(figures.observed_at_ns).i128(*at_ns);
            write_figure(w, figures.leverage_scaled);
            write_figure(w, figures.margin_use_scaled);
            write_figure(w, figures.equity_scaled);
        }
        ControlEvent::DayOpened { day, unrealized_scaled: figure, at_ns }
        | ControlEvent::DayPeak { day, result_scaled: figure, at_ns } => {
            w.u64(*day).i128(*figure).i128(*at_ns);
        }
        ControlEvent::AccountCharged { venue, charge, at_ns } => {
            w.str(venue)?.str(&charge.charge_id)?.str(&charge.fee.asset)?;
            w.i128(charge.fee.amount_scaled).u8(charge.fee.scale).i128(*at_ns);
        }
        ControlEvent::ScopesValued { figures, at_ns } => {
            w.u64(figures.len() as u64);
            for figure in figures {
                write_scope(w, &figure.scope)?;
                w.u8(kind_code(figure.kind));
                w.i128(figure.value_scaled);
            }
            w.i128(*at_ns);
        }
        _ => {}
    }
    Ok(())
}

/// The classes that move an order, split from `write_fields` to keep each under its size bound.
fn write_order_fields(w: &mut Writer, event: &ControlEvent) -> Result<(), EncodeError> {
    match event {
        ControlEvent::Intent { .. } => write_intent(w, event)?,
        ControlEvent::Filled { client_id, fill, at_ns } => {
            write_fill(w.str(client_id)?, fill)?;
            w.i128(*at_ns);
        }
        ControlEvent::Reconciled { venue, client_id, settlement, at_ns } => {
            write_reconciled(w, venue, client_id, settlement, *at_ns)?;
        }
        ControlEvent::ReconciliationCompleted { venue, verdict, findings, evidence, at_ns } => {
            write_completed(w, venue, *verdict, findings, *evidence, *at_ns)?;
        }
        ControlEvent::Settled { client_id, outcome, at_ns } => {
            w.str(client_id)?;
            write_outcome(w, outcome)?;
            w.i128(*at_ns);
        }
        ControlEvent::Withdrawn { client_id, instrument, outcome, at_ns } => {
            w.str(client_id)?;
            w.str(instrument.as_str())?;
            write_outcome(w, outcome)?;
            w.i128(*at_ns);
        }
        ControlEvent::InstrumentScales {
            venue,
            instrument,
            price_scale,
            quantity_scale,
            at_ns,
            identity,
            quote_currency,
        } => {
            w.str(venue)?.str(instrument.as_str())?;
            w.u8(*price_scale).u8(*quantity_scale).i128(*at_ns);
            write_present(w, identity.as_deref())?;
            write_present(w, quote_currency.as_deref())?;
        }
        // Every other class is written by `write_fields`, which routes only the seven above here.
        _ => {}
    }
    Ok(())
}

fn write_refusal(w: &mut Writer, reason: &RefusalReason) -> Result<(), EncodeError> {
    match reason {
        RefusalReason::NotActive { state } => {
            w.u8(REFUSED_NOT_ACTIVE).u8(state_code(*state));
        }
        RefusalReason::LimitsUnrecovered => {
            w.u8(REFUSED_UNRECOVERED);
        }
        RefusalReason::Breached => {
            w.u8(REFUSED_BREACHED);
        }
        RefusalReason::Unjudged { kind } => {
            w.u8(REFUSED_UNJUDGED).u8(kind_code(*kind));
        }
        RefusalReason::Stale { age_ns } => {
            w.u8(REFUSED_STALE).i128(*age_ns);
        }
        RefusalReason::ClockSkew { skew_ns } => {
            w.u8(REFUSED_SKEW).i128(*skew_ns);
        }
        RefusalReason::NoBudget { scope, kind } => {
            w.u8(REFUSED_NO_BUDGET);
            write_scope(w, scope)?;
            w.u8(kind_code(*kind));
        }
        RefusalReason::NotAdmitted { detail } => {
            w.u8(REFUSED_NOT_ADMITTED);
            w.str(detail)?;
        }
        RefusalReason::Unreadable => {
            w.u8(REFUSED_UNREADABLE);
        }
        RefusalReason::DoesNotReduce { held_scaled, working_scaled } => {
            w.u8(REFUSED_DOES_NOT_REDUCE).i128(*held_scaled).i128(*working_scaled);
        }
        RefusalReason::RateLimited { retry_after_ns } => {
            w.u8(REFUSED_RATE_LIMITED).i128(*retry_after_ns);
        }
        RefusalReason::Uncovered { held_scaled, working_scaled } => {
            w.u8(REFUSED_UNCOVERED).i128(*held_scaled).i128(*working_scaled);
        }
        RefusalReason::InstrumentRefused { symbol, cause } => {
            w.u8(REFUSED_INSTRUMENT);
            w.str(symbol.as_str())?;
            w.u8(match cause {
                InstrumentRefusal::Unknown => INSTRUMENT_UNKNOWN,
                InstrumentRefusal::Halted => INSTRUMENT_HALTED,
                InstrumentRefusal::Delisted => INSTRUMENT_DELISTED,
                InstrumentRefusal::BelowMinimumSize => INSTRUMENT_BELOW_MINIMUM,
            });
        }
    }
    Ok(())
}

fn write_book(w: &mut Writer, book: Option<&CapturedBook>) {
    match book {
        Some(b) => {
            w.u8(PRESENT)
                .i64(b.best_bid_ticks)
                .i64(b.best_bid_qty_lots)
                .i64(b.best_ask_ticks)
                .i64(b.best_ask_qty_lots)
                .i128(b.observed_at_ns)
                .i128(b.capture_lag_ns);
        }
        None => {
            w.u8(ABSENT);
        }
    }
}

/// Client id, venue symbol, side, quantity, optional limit price and the reduce-only flag.
fn write_request(w: &mut Writer, request: &OrderRequest) -> Result<(), EncodeError> {
    w.str(&request.client_id)?;
    w.str(request.instrument.as_str())?;
    w.u8(match request.side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    });
    w.i128(request.quantity_scaled);
    match request.price_scaled {
        Some(p) => {
            w.u8(PRESENT).i128(p);
        }
        None => {
            w.u8(ABSENT);
        }
    }
    w.u8(u8::from(request.reduce_only));
    Ok(())
}

fn write_participation(w: &mut Writer, participation: &Participation) -> Result<(), EncodeError> {
    match participation {
        Participation::Participating => {
            w.u8(PARTICIPATION_JOINED);
            w.str("")?;
        }
        Participation::Withdrawn { reason } => {
            w.u8(PARTICIPATION_WITHDRAWN);
            w.str(reason)?;
        }
    }
    Ok(())
}

fn write_optional_scope(w: &mut Writer, scope: Option<&Scope>) -> Result<(), EncodeError> {
    match scope {
        Some(s) => {
            w.u8(PRESENT);
            write_scope(w, s)?;
        }
        None => {
            w.u8(ABSENT);
        }
    }
    Ok(())
}

/// The after-figure trails its tag so the figureless shape simply ends the payload.
fn write_postcondition(w: &mut Writer, outcome: &PostconditionOutcome) {
    match outcome {
        PostconditionOutcome::Held { after } => {
            w.u8(POSTCONDITION_HELD);
            write_threshold(w, *after);
        }
        PostconditionOutcome::Increased { after } => {
            w.u8(POSTCONDITION_INCREASED);
            write_threshold(w, *after);
        }
        PostconditionOutcome::Unverifiable => {
            w.u8(POSTCONDITION_UNVERIFIABLE);
        }
    }
}

/// The decision a benchmark belongs to and the book at its instant.
fn write_decision(
    w: &mut Writer,
    correlation_id: &str,
    instrument: &VenueSymbol,
    decided_at_ns: i128,
    book: Option<&CapturedBook>,
) -> Result<(), EncodeError> {
    w.str(correlation_id)?;
    w.str(instrument.as_str())?;
    w.i128(decided_at_ns);
    write_book(w, book);
    Ok(())
}

/// The role a command was issued under, the action asked for and the trading state read back.
fn write_authority(
    w: &mut Writer,
    role: Role,
    action: Action,
    state: SafeState,
) -> Result<(), EncodeError> {
    w.str(role.as_str())?;
    w.str(action.as_str())?;
    w.u8(state_code(state));
    Ok(())
}
