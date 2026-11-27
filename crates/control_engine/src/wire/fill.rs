// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The layout of a fill inside `OrderFilled`. Every optional field carries a presence flag. A
//! figure the venue did not state is absent on decode, never zero.

use bijective_lifecycle::{Fee, Fill, Liquidity};
use bijective_model::Side;
use bijective_model::market::ExchangeTime;
use bijective_serialization::codec::{DecodeError, Reader, Writer, writer::EncodeError};

use super::codes::{ABSENT, PRESENT, SIDE_BUY, SIDE_SELL, read_flag};

const LIQUIDITY_MAKER: u8 = 0;
const LIQUIDITY_TAKER: u8 = 1;
const LIQUIDITY_NOT_PUBLISHED: u8 = 2;

fn flag(w: &mut Writer, present: bool) {
    w.u8(if present { PRESENT } else { ABSENT });
}

pub(crate) fn write_fill(w: &mut Writer, fill: &Fill) -> Result<(), EncodeError> {
    w.str(&fill.fill_id)?;
    flag(w, fill.venue_order_id.is_some());
    if let Some(id) = &fill.venue_order_id {
        w.str(id)?;
    }
    w.u8(match fill.side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    });
    w.i128(fill.price_scaled).i128(fill.quantity_scaled);
    flag(w, fill.fee.is_some());
    if let Some(fee) = &fill.fee {
        w.str(&fee.asset)?;
        w.i128(fee.amount_scaled).u8(fee.scale);
    }
    w.u8(match fill.liquidity {
        Liquidity::Maker => LIQUIDITY_MAKER,
        Liquidity::Taker => LIQUIDITY_TAKER,
        Liquidity::NotPublished => LIQUIDITY_NOT_PUBLISHED,
    });
    match fill.exchange_time {
        ExchangeTime::At { at_ns } => {
            flag(w, true);
            w.i128(at_ns);
        }
        ExchangeTime::NotPublished => flag(w, false),
    }
    w.i128(fill.received_at_ns);
    Ok(())
}

pub(crate) fn read_fill(r: &mut Reader<'_>) -> Result<Fill, DecodeError> {
    let fill_id = r.str()?.to_string();
    let venue_order_id =
        if read_flag(r, "fill.venue_order_id")? { Some(r.str()?.to_string()) } else { None };
    let side = match r.u8()? {
        SIDE_BUY => Side::Buy,
        SIDE_SELL => Side::Sell,
        found => {
            return Err(DecodeError::InvalidValue { field: "fill.side", found: u64::from(found) });
        }
    };
    let price_scaled = r.i128()?;
    let quantity_scaled = r.i128()?;
    let fee = if read_flag(r, "fill.fee")? {
        let asset = r.str()?.to_string();
        Some(Fee { asset, amount_scaled: r.i128()?, scale: r.u8()? })
    } else {
        None
    };
    let liquidity = match r.u8()? {
        LIQUIDITY_MAKER => Liquidity::Maker,
        LIQUIDITY_TAKER => Liquidity::Taker,
        LIQUIDITY_NOT_PUBLISHED => Liquidity::NotPublished,
        found => {
            return Err(DecodeError::InvalidValue {
                field: "fill.liquidity",
                found: u64::from(found),
            });
        }
    };
    let exchange_time = if read_flag(r, "fill.exchange_time")? {
        ExchangeTime::At { at_ns: r.i128()? }
    } else {
        ExchangeTime::NotPublished
    };
    Ok(Fill {
        fill_id,
        venue_order_id,
        side,
        price_scaled,
        quantity_scaled,
        fee,
        liquidity,
        exchange_time,
        received_at_ns: r.i128()?,
    })
}
