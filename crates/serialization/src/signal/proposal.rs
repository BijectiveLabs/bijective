// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The proposal's class layout.
//! Wire discriminants are pinned constants, never derived from declaration order.

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_model::{InstrumentId, OrderTerms, Proposal, Side, TimeInForce};

/// The layout version this build writes and is the newest it reads. Each appends fields an
/// earlier record lacks and reads as unsaid: 2 the market position (not position zero), 3 the
/// venue, 4 the strategy, 5 the order terms (decision 0009, the default terms), 6 the stop and
/// the reduce-only flag (decision 0012), 7 the parent and the one-cancels-other group (0013), 8 the
/// expiry (0014).
pub const VERSION: u16 = 8;

const SIDE_BUY: u8 = 0;
const SIDE_SELL: u8 = 1;

const ABSENT: u8 = 0;
const PRESENT: u8 = 1;

fn write_optional(w: &mut Writer, value: Option<i128>) {
    match value {
        Some(v) => {
            w.u8(PRESENT).i128(v);
        }
        None => {
            w.u8(ABSENT);
        }
    }
}

fn read_optional(r: &mut Reader<'_>, field: &'static str) -> Result<Option<i128>, DecodeError> {
    match r.u8()? {
        ABSENT => Ok(None),
        PRESENT => Ok(Some(r.i128()?)),
        other => Err(DecodeError::InvalidValue { field, found: other as u64 }),
    }
}

fn write_text(w: &mut Writer, value: Option<&str>) -> Result<(), EncodeError> {
    match value {
        Some(text) => {
            w.u8(PRESENT);
            w.str(text)?;
        }
        None => {
            w.u8(ABSENT);
        }
    }
    Ok(())
}

/// A text field appended after version 2: absent at the end of an older record.
fn read_text(r: &mut Reader<'_>, field: &'static str) -> Result<Option<String>, DecodeError> {
    if !r.more() {
        return Ok(None);
    }
    match r.u8()? {
        PRESENT => Ok(Some(r.str()?.to_string())),
        ABSENT => Ok(None),
        other => Err(DecodeError::InvalidValue { field, found: u64::from(other) }),
    }
}

pub fn encode(proposal: &Proposal) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::ProposalEmitted, VERSION);
    w.str(&proposal.desk)?;
    w.str(proposal.instrument.as_str())?;
    w.str(&proposal.client_id)?;
    w.u8(match proposal.side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    });
    w.i128(proposal.quantity_scaled);
    write_optional(&mut w, proposal.price_scaled);
    write_optional(&mut w, proposal.cap_scaled);
    w.i128(proposal.decided_at_ns);
    match proposal.decided_at_position {
        Some(at) => {
            w.u8(PRESENT).u64(at);
        }
        None => {
            w.u8(ABSENT).u64(0);
        }
    }
    write_text(&mut w, proposal.venue.as_deref())?;
    write_text(&mut w, proposal.strategy.as_deref())?;
    w.u8(proposal.terms.time_in_force.code()).u8(u8::from(proposal.terms.post_only));
    write_stop(&mut w, proposal.terms.stop_price_scaled);
    w.u8(u8::from(proposal.reduce_only));
    write_text(&mut w, proposal.parent.as_deref())?;
    write_text(&mut w, proposal.oco.as_deref())?;
    write_stop(&mut w, proposal.terms.expire_at_ns);
    Ok(w.finish())
}

/// Decode a payload. An unknown class or a newer version is refused, never skipped.
pub fn decode(payload: &[u8]) -> Result<Proposal, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    // Refused before a field is read, so the error names the real fault: a payload that was never ours.
    if envelope.class != ClassId::ProposalEmitted {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let desk = r.str()?.to_string();
    let instrument = InstrumentId::new(r.str()?);
    let client_id = r.str()?.to_string();
    let side = match r.u8()? {
        SIDE_BUY => Side::Buy,
        SIDE_SELL => Side::Sell,
        other => {
            return Err(DecodeError::InvalidValue { field: "side", found: other as u64 });
        }
    };
    let quantity_scaled = r.i128()?;
    let price_scaled = read_optional(&mut r, "price_present")?;
    let cap_scaled = read_optional(&mut r, "cap_present")?;
    let decided_at_ns = r.i128()?;
    // Appended at version 2. A record without it was written before a strategy could say.
    let decided_at_position = if r.more() {
        match r.u8()? {
            PRESENT => Some(r.u64()?),
            ABSENT => {
                let _ = r.u64()?;
                None
            }
            other => {
                return Err(DecodeError::InvalidValue {
                    field: "decided_at_position",
                    found: u64::from(other),
                });
            }
        }
    } else {
        None
    };
    // Appended at version 3. A record without it was written before a strategy could name one.
    let venue = read_text(&mut r, "venue")?;
    // Appended at version 4. A record without it was written before proposals named a strategy.
    let strategy = read_text(&mut r, "strategy")?;
    let mut terms = read_terms(&mut r)?;
    let reduce_only = read_reduce_only(&mut r)?;
    // Appended at version 7. A record without them was written before proposals were linked.
    let parent = read_text(&mut r, "parent")?;
    let oco = read_text(&mut r, "oco")?;
    // Appended at version 8. A record without it does not expire.
    terms.expire_at_ns = read_stop(&mut r, "expire_at_ns")?;
    r.finish()?;

    Ok(Proposal {
        desk,
        instrument,
        side,
        quantity_scaled,
        price_scaled,
        cap_scaled,
        client_id,
        decided_at_position,
        decided_at_ns,
        venue,
        strategy,
        terms,
        reduce_only,
        parent,
        oco,
    })
}

/// Appended at version 6. A record without it does not reduce only.
fn read_reduce_only(r: &mut Reader<'_>) -> Result<bool, DecodeError> {
    match if r.more() { r.u8()? } else { 0 } {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(DecodeError::InvalidValue { field: "reduce_only", found: u64::from(other) }),
    }
}

/// Appended at version 5. A record without it was written before a proposal stated terms.
fn read_terms(r: &mut Reader<'_>) -> Result<OrderTerms, DecodeError> {
    if !r.more() {
        return Ok(OrderTerms::default());
    }
    let code = r.u8()?;
    let time_in_force = TimeInForce::from_code(code)
        .ok_or(DecodeError::InvalidValue { field: "time_in_force", found: u64::from(code) })?;
    let post_only = match r.u8()? {
        0 => false,
        1 => true,
        other => {
            return Err(DecodeError::InvalidValue { field: "post_only", found: u64::from(other) });
        }
    };
    let stop_price_scaled = read_stop(r, "stop_price")?;
    Ok(OrderTerms { time_in_force, post_only, stop_price_scaled, expire_at_ns: None })
}

/// An optional price or instant as a presence flag then the value. Nothing at all on a record
/// written before the field.
pub fn write_stop(w: &mut Writer, stop_price_scaled: Option<i128>) {
    match stop_price_scaled {
        Some(price) => {
            w.u8(1).i128(price);
        }
        None => {
            w.u8(0);
        }
    }
}

/// Reads what `write_stop` wrote. The end of the payload is a record from before stops.
pub fn read_stop(r: &mut Reader<'_>, field: &'static str) -> Result<Option<i128>, DecodeError> {
    if !r.more() {
        return Ok(None);
    }
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(r.i128()?)),
        other => Err(DecodeError::InvalidValue { field, found: u64::from(other) }),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/signal/proposal_tests.rs"]
mod tests;
