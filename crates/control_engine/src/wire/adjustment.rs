// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The layout of `PositionAdjusted`: who accepted which position change, on which venue, and why.

use bijective_model::{Owner, Side, VenueSymbol};
use bijective_serialization::codec::{DecodeError, Reader, Writer, writer::EncodeError};

use super::ControlEvent;
use super::codes::{SIDE_BUY, SIDE_SELL, read_flag};

/// The fields of `ControlEvent::PositionAdjusted`, kept together so the event stays one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adjustment {
    pub venue: String,
    /// The command that asked for it. Also the change's identity in the position's record.
    pub adjustment_id: String,
    pub actor: String,
    pub instrument: VenueSymbol,
    pub side: Side,
    pub quantity_scaled: i128,
    pub price_scaled: i128,
    pub reason: String,
    /// The desk and strategy the moved position belongs to. `None` on an adjustment that named
    /// none, or recorded before layout version 6.
    pub owner: Option<Owner>,
}

pub(crate) fn write_adjustment(
    w: &mut Writer,
    a: &Adjustment,
    at_ns: i128,
) -> Result<(), EncodeError> {
    w.str(&a.venue)?.str(&a.adjustment_id)?.str(&a.actor)?.str(a.instrument.as_str())?;
    w.u8(match a.side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    });
    w.i128(a.quantity_scaled).i128(a.price_scaled);
    w.str(&a.reason)?;
    w.i128(at_ns);
    // Appended at layout 6, after the time, so an older record ends where it always did.
    w.u8(u8::from(a.owner.is_some()));
    if let Some(owner) = &a.owner {
        w.str(&owner.desk)?;
        w.u8(u8::from(owner.strategy.is_some()));
        if let Some(strategy) = &owner.strategy {
            w.str(strategy)?;
        }
    }
    Ok(())
}

pub(crate) fn decode_position_adjusted(r: &mut Reader<'_>) -> Result<ControlEvent, DecodeError> {
    let venue = r.str()?.to_string();
    let adjustment_id = r.str()?.to_string();
    let actor = r.str()?.to_string();
    let instrument = VenueSymbol::new(r.str()?);
    let side = match r.u8()? {
        SIDE_BUY => Side::Buy,
        SIDE_SELL => Side::Sell,
        other => return Err(DecodeError::InvalidValue { field: "side", found: u64::from(other) }),
    };
    let quantity_scaled = r.i128()?;
    let price_scaled = r.i128()?;
    let reason = r.str()?.to_string();
    let at_ns = r.i128()?;
    let owner = if r.more() && read_flag(r, "adjustment.owner")? {
        let desk = r.str()?.to_string();
        let strategy =
            if read_flag(r, "adjustment.strategy")? { Some(r.str()?.to_string()) } else { None };
        Some(Owner { desk, strategy })
    } else {
        None
    };
    let adjustment = Adjustment {
        venue,
        adjustment_id,
        actor,
        instrument,
        side,
        quantity_scaled,
        price_scaled,
        reason,
        owner,
    };
    Ok(ControlEvent::PositionAdjusted { adjustment, at_ns })
}
