// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue evidence read from the venue's own records: its orders, its trades and the positions its
//! data service holds. Pure over the answers, so captured bodies are checked offline.
//!
//! The venue carries no client identifier, so an order is linked to the platform only through the
//! order hashes this client signed. A row that cannot be judged is refused with the field that
//! failed and marks the evidence incomplete. It is never dropped. Field meanings and the questions
//! still open: `docs/integrations/polymarket.md`, "Execution evidence".

use std::collections::BTreeMap;

use bijective_lifecycle::{Fill, FillReport, Liquidity, OrderStatusReport, ReportedStatus};
use bijective_model::market::ExchangeTime;
use bijective_model::{PositionSnapshot, Side, VenueSymbol};
use bijective_venue::normalise::{Scale, parse_scaled};
use serde_json::Value;

/// The cursor the venue writes when no page follows (Manage orders).
const LAST_CURSOR: &str = "LTE=";

/// The platform's client identifier for each order hash this client signed, keyed lowercase.
pub type ClientOf = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy)]
pub struct Scales {
    pub price: Scale,
    pub quantity: Scale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowRefused {
    pub field: &'static str,
    pub detail: String,
}

impl core::fmt::Display for RowRefused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.field, self.detail)
    }
}

fn refused(field: &'static str, detail: impl Into<String>) -> RowRefused {
    RowRefused { field, detail: detail.into() }
}

fn text<'a>(row: &'a Value, field: &'static str) -> Result<&'a str, RowRefused> {
    row.get(field).and_then(Value::as_str).ok_or_else(|| refused(field, "absent or not text"))
}

fn scaled(row: &Value, field: &'static str, scale: Scale) -> Result<i128, RowRefused> {
    let value = text(row, field)?;
    parse_scaled(value, scale).map_err(|e| refused(field, format!("{value}: {e:?}")))
}

fn side(row: &Value) -> Result<Side, RowRefused> {
    match text(row, "side")?.to_ascii_uppercase().as_str() {
        "BUY" => Ok(Side::Buy),
        "SELL" => Ok(Side::Sell),
        other => Err(refused("side", format!("{other} is neither BUY nor SELL"))),
    }
}

/// Whole seconds, written as a number or as digits.
fn seconds(value: Option<&Value>) -> Option<i128> {
    let value = value?;
    let whole = value.as_u64().or_else(|| value.as_str().and_then(|t| t.parse::<u64>().ok()))?;
    Some(i128::from(whole) * 1_000_000_000)
}

fn client_of(clients: &ClientOf, hash: &str) -> Option<String> {
    clients.get(&hash.to_ascii_lowercase()).cloned()
}

/// One page of a CLOB list. The reference writes an object with `data` and `next_cursor`. The
/// guide shows a bare array. Both are read, and anything else is refused.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub rows: Vec<Value>,
    pub next_cursor: Option<String>,
}

pub fn read_page(body: &str) -> Result<Page, RowRefused> {
    let parsed: Value = serde_json::from_str(body).map_err(|_| refused("page", "not JSON"))?;
    if let Value::Array(rows) = parsed {
        return Ok(Page { rows, next_cursor: None });
    }
    let rows = parsed
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("data", "absent or not a list"))?
        .clone();
    let next_cursor = parsed
        .get("next_cursor")
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty() && *cursor != LAST_CURSOR)
        .map(str::to_string);
    Ok(Page { rows, next_cursor })
}

/// The statuses the reference lists for an order (`OpenOrder`). Another is refused, not guessed.
fn order_status(status: &str) -> Result<ReportedStatus, RowRefused> {
    match status.to_ascii_uppercase().as_str() {
        "LIVE" => Ok(ReportedStatus::Working),
        "MATCHED" => Ok(ReportedStatus::Filled),
        "CANCELED" | "CANCELED_MARKET_RESOLVED" => Ok(ReportedStatus::Canceled),
        "INVALID" => Ok(ReportedStatus::Rejected),
        _ => Err(refused("status", format!("{status} is not an order status this build reads"))),
    }
}

/// One order row. Sizes are decimal shares, already normalised by the venue (`OpenOrder`).
pub fn order_report(
    row: &Value,
    clients: &ClientOf,
    scales: Scales,
) -> Result<OrderStatusReport, RowRefused> {
    let hash = text(row, "id")?;
    Ok(OrderStatusReport {
        client_id: client_of(clients, hash),
        venue_order_id: hash.to_ascii_lowercase(),
        instrument: VenueSymbol::new(text(row, "asset_id")?),
        side: side(row)?,
        ordered_quantity_scaled: scaled(row, "original_size", scales.quantity)?,
        filled_quantity_scaled: scaled(row, "size_matched", scales.quantity)?,
        status: order_status(text(row, "status")?)?,
        reported_at: seconds(row.get("created_at"))
            .map_or(ExchangeTime::NotPublished, |at_ns| ExchangeTime::At { at_ns }),
    })
}

/// What one trade row says about this wallet's orders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeReading {
    pub fills: Vec<FillReport>,
    /// Only `CONFIRMED` is final: a matched trade can still fail on chain (Order lifecycle).
    pub settled: bool,
}

/// A trade's settlement, written with or without the `TRADE_STATUS_` prefix (the reference and
/// the guide differ). `None` for a failed trade.
fn trade_settled(status: &str) -> Result<Option<bool>, RowRefused> {
    let upper = status.to_ascii_uppercase();
    match upper.strip_prefix("TRADE_STATUS_").unwrap_or(&upper) {
        "CONFIRMED" => Ok(Some(true)),
        "MATCHED" | "MINED" | "RETRYING" => Ok(Some(false)),
        "FAILED" => Ok(None),
        _ => Err(refused("status", format!("{status} is not a trade status this build reads"))),
    }
}

/// The venue's match instant: nanoseconds where published, else whole seconds.
fn matched_at(row: &Value) -> ExchangeTime {
    let nanos = row.get("match_time_nano").and_then(Value::as_str).and_then(|t| t.parse().ok());
    match nanos.or_else(|| seconds(row.get("match_time"))) {
        Some(at_ns) => ExchangeTime::At { at_ns },
        None => ExchangeTime::NotPublished,
    }
}

/// The context every fill of one trade row shares.
struct TradeContext<'a> {
    clients: &'a ClientOf,
    scales: Scales,
    exchange_time: ExchangeTime,
    received_at_ns: i128,
}

/// A fill from the order-level fields `order`, which is the trade row for a taker and one entry of
/// `maker_orders` for a maker. The fee is not stated as an amount, only as a rate, so none is
/// carried.
fn fill(
    order: &Value,
    order_field: &'static str,
    quantity_field: &'static str,
    fill_id: String,
    liquidity: Liquidity,
    context: &TradeContext<'_>,
) -> Result<FillReport, RowRefused> {
    let hash = text(order, order_field)?;
    Ok(FillReport {
        client_id: client_of(context.clients, hash),
        instrument: VenueSymbol::new(text(order, "asset_id")?),
        fill: Fill {
            fill_id,
            venue_order_id: Some(hash.to_ascii_lowercase()),
            side: side(order)?,
            price_scaled: scaled(order, "price", context.scales.price)?,
            quantity_scaled: scaled(order, quantity_field, context.scales.quantity)?,
            fee: None,
            liquidity,
            exchange_time: context.exchange_time,
            received_at_ns: context.received_at_ns,
        },
    })
}

/// One trade row read for `wallet`. As taker the row is this wallet's own order. As maker this
/// wallet's orders are the `maker_orders` entries it made, each with its own asset, side and
/// amount, because a maker can sit on the complementary outcome.
pub fn trade_reading(
    row: &Value,
    wallet: &str,
    clients: &ClientOf,
    scales: Scales,
    received_at_ns: i128,
) -> Result<TradeReading, RowRefused> {
    let trade_id = text(row, "id")?;
    // Refused so an operator sees it: a fill read from this trade while it was matched is not
    // undone by the failure, and only an operator can judge what the chain kept.
    let Some(settled) = trade_settled(text(row, "status")?)? else {
        return Err(refused("status", format!("trade {trade_id} failed on chain")));
    };
    let context = TradeContext { clients, scales, exchange_time: matched_at(row), received_at_ns };
    let fills = match text(row, "trader_side")?.to_ascii_uppercase().as_str() {
        "TAKER" => vec![fill(
            row,
            "taker_order_id",
            "size",
            trade_id.to_string(),
            Liquidity::Taker,
            &context,
        )?],
        "MAKER" => maker_fills(row, trade_id, wallet, &context)?,
        other => return Err(refused("trader_side", format!("{other} is neither TAKER nor MAKER"))),
    };
    Ok(TradeReading { fills, settled })
}

fn maker_fills(
    row: &Value,
    trade_id: &str,
    wallet: &str,
    context: &TradeContext<'_>,
) -> Result<Vec<FillReport>, RowRefused> {
    let entries = row
        .get("maker_orders")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("maker_orders", "absent or not a list"))?;
    let mut fills = Vec::new();
    for entry in entries {
        if !text(entry, "maker_address")?.eq_ignore_ascii_case(wallet) {
            continue;
        }
        // One trade can match several of this wallet's orders. The order keeps each fill distinct.
        let fill_id = format!("{trade_id}:{}", text(entry, "order_id")?.to_ascii_lowercase());
        fills.push(fill(entry, "order_id", "matched_amount", fill_id, Liquidity::Maker, context)?);
    }
    if fills.is_empty() {
        return Err(refused("maker_orders", "no entry was made by this wallet"));
    }
    Ok(fills)
}

/// Decision 0002, rule 6: a trade matched and not yet confirmed is not a fill, and its order stays
/// one that may execute. The venue's matched size already counts such a trade, so `pending`, the
/// unconfirmed quantity per order hash, is taken back out, and an order the venue calls matched
/// is reported working until its trades confirm.
pub fn hold_unconfirmed(orders: &mut [OrderStatusReport], pending: &BTreeMap<String, i128>) {
    for order in orders {
        let Some(quantity) = pending.get(&order.venue_order_id) else { continue };
        order.filled_quantity_scaled = (order.filled_quantity_scaled - quantity).max(0);
        if order.status == ReportedStatus::Filled {
            order.status = ReportedStatus::Working;
        }
    }
}

/// One page of the data service's positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionPage {
    pub positions: Vec<PositionSnapshot>,
    pub next_cursor: Option<String>,
}

/// A number the data service writes in binary floating point, as decimal text at `digits` places.
/// `exact` refuses a value with more places than that instead of rounding it. One place past
/// `digits` is examined: a double carries holdings below about 3e8 shares exactly to that place.
fn fixed(row: &Value, field: &'static str, digits: u8, exact: bool) -> Result<String, RowRefused> {
    let value = row
        .get(field)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| refused(field, "absent or not a finite number"))?;
    let wider = format!("{value:.*}", usize::from(digits) + 1);
    if exact && !wider.ends_with('0') {
        return Err(refused(field, format!("{value} is finer than {digits} places")));
    }
    Ok(format!("{value:.*}", usize::from(digits)))
}

/// Holdings in shares. The quantity must be exact at the quantity scale. The entry price is the
/// venue's weighted average and is rounded to the price scale, since it values and never sizes.
pub fn position_page(body: &str, scales: Scales) -> Result<PositionPage, RowRefused> {
    let parsed: Value = serde_json::from_str(body).map_err(|_| refused("page", "not JSON"))?;
    let rows = parsed
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("data", "absent or not a list"))?;
    let mut positions = Vec::with_capacity(rows.len());
    for row in rows {
        let size = fixed(row, "current_size", scales.quantity.digits(), true)?;
        let price = fixed(row, "avg_price", scales.price.digits(), false)?;
        positions.push(PositionSnapshot {
            instrument: VenueSymbol::new(text(row, "token_id")?),
            quantity_scaled: parse_scaled(&size, scales.quantity)
                .map_err(|e| refused("current_size", format!("{size}: {e:?}")))?,
            entry_price_scaled: parse_scaled(&price, scales.price)
                .map_err(|e| refused("avg_price", format!("{price}: {e:?}")))?,
        });
    }
    let pagination = parsed.get("pagination");
    let has_more = pagination.and_then(|p| p.get("has_more")).and_then(Value::as_bool);
    let next_cursor = match has_more {
        Some(true) => Some(
            pagination
                .and_then(|p| p.get("next_cursor"))
                .and_then(Value::as_str)
                .ok_or_else(|| refused("next_cursor", "more pages and no cursor"))?
                .to_string(),
        ),
        Some(false) => None,
        None => return Err(refused("has_more", "absent: the read cannot tell it is whole")),
    };
    Ok(PositionPage { positions, next_cursor })
}

#[cfg(test)]
#[path = "../tests/unit/evidence_tests.rs"]
mod tests;
