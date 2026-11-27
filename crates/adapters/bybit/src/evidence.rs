// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue evidence from the venue's own records: order, execution and position rows, read the same
//! way whether they arrive from the signed REST queries or the private stream.
//!
//! Each row either becomes a platform report or is refused with the field that failed. A refused
//! row is never dropped: the evidence it belonged to is marked incomplete, so reconciliation can
//! never call a partial picture aligned. Field meanings: `docs/integrations/bybit.md`, section
//! "Execution evidence".

use bijective_lifecycle::{
    AccountCharge, Fee, Fill, FillReport, Liquidity, OrderStatusReport, PositionEvidence,
    PositionReport, ReportedStatus, VenueEvidence,
};
use bijective_model::market::ExchangeTime;
use bijective_model::{AccountMargin, Side, VenueSymbol};
use bijective_venue::normalise::{Scale, parse_scaled};
pub use bijective_venue::profile::{InstrumentScales, Scales};
use serde_json::Value;

/// Fees arrive with more places than prices (the documented examples carry six and seven). Twelve
/// holds them exactly, and anything finer is refused rather than rounded.
pub const FEE_SCALE: u8 = 12;

/// The only product this adapter trades. A row of another category is refused, not reinterpreted.
pub const CATEGORY: &str = "linear";

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

/// What one execution row is. Only executions that move a position are fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Execution {
    /// Boxed: funding carries nothing.
    Fill(Box<FillReport>),
    /// A funding payment: cash, not quantity. A charge to the account, never applied to a position.
    Funding(AccountCharge),
}

/// A row of an instrument the profile does not list cannot be this platform's. It is read at
/// this many places so only its presence is judged, never refused for its digits (decision 0007).
pub const OUTSIDE_CATALOGUE_PLACES: u8 = 12;

/// The scales of the instrument a row names: the profile's, or the outside-catalogue reading.
pub fn scales_of(row: &Value, scales: &InstrumentScales) -> Result<Scales, RowRefused> {
    if let Some(listed) = scales.of(&VenueSymbol::new(text(row, "symbol")?)) {
        return Ok(listed);
    }
    let wide = Scale::new(OUTSIDE_CATALOGUE_PLACES)
        .map_err(|e| RowRefused { field: "symbol", detail: e.to_string() })?;
    Ok(Scales { price: wide, quantity: wide })
}

fn text<'a>(row: &'a Value, field: &'static str) -> Result<&'a str, RowRefused> {
    row.get(field)
        .and_then(Value::as_str)
        .ok_or(RowRefused { field, detail: "absent or not text".into() })
}

fn scaled(row: &Value, field: &'static str, scale: Scale) -> Result<i128, RowRefused> {
    parse_scaled(text(row, field)?, scale)
        .map_err(|e| RowRefused { field, detail: format!("{e:?}") })
}

fn millis(row: &Value, field: &'static str) -> Result<ExchangeTime, RowRefused> {
    text(row, field)?
        .parse::<u64>()
        .map(ExchangeTime::from_ms)
        .map_err(|_| RowRefused { field, detail: "not a millisecond timestamp".into() })
}

fn side(row: &Value) -> Result<Side, RowRefused> {
    match text(row, "side")? {
        "Buy" => Ok(Side::Buy),
        "Sell" => Ok(Side::Sell),
        other => Err(RowRefused { field: "side", detail: format!("{other} is not Buy or Sell") }),
    }
}

/// An empty `orderLinkId` is an order this platform did not identify.
fn client_id(row: &Value) -> Result<Option<String>, RowRefused> {
    let id = text(row, "orderLinkId")?;
    Ok((!id.is_empty()).then(|| id.to_string()))
}

/// Refused when present and not this adapter's category. REST rows carry it at the response level.
pub fn check_category(row: &Value) -> Result<(), RowRefused> {
    match row.get("category").and_then(Value::as_str) {
        None | Some(CATEGORY) => Ok(()),
        Some(other) => {
            Err(RowRefused { field: "category", detail: format!("{other} is not {CATEGORY}") })
        }
    }
}

/// Open statuses are working. `Cancelled` in derivatives may carry executed quantity, which the
/// lifecycle keeps.
fn status(row: &Value) -> Result<ReportedStatus, RowRefused> {
    match text(row, "orderStatus")? {
        "New" | "PartiallyFilled" | "Untriggered" | "Triggered" => Ok(ReportedStatus::Working),
        "Filled" => Ok(ReportedStatus::Filled),
        "Cancelled" | "PartiallyFilledCanceled" | "Deactivated" => Ok(ReportedStatus::Canceled),
        "Rejected" => Ok(ReportedStatus::Rejected),
        other => Err(RowRefused {
            field: "orderStatus",
            detail: format!("{other} is not a documented status"),
        }),
    }
}

pub fn order_report(
    row: &Value,
    scales: &InstrumentScales,
) -> Result<OrderStatusReport, RowRefused> {
    check_category(row)?;
    let scales = scales_of(row, scales)?;
    Ok(OrderStatusReport {
        client_id: client_id(row)?,
        venue_order_id: text(row, "orderId")?.to_string(),
        instrument: VenueSymbol::new(text(row, "symbol")?),
        side: side(row)?,
        ordered_quantity_scaled: scaled(row, "qty", scales.quantity)?,
        filled_quantity_scaled: scaled(row, "cumExecQty", scales.quantity)?,
        status: status(row)?,
        reported_at: millis(row, "updatedTime")?,
    })
}

/// Executions that change a position. Others are refused by name until their meaning is mapped.
fn moves_position(exec_type: &str) -> bool {
    matches!(exec_type, "Trade" | "BlockTrade" | "AdlTrade" | "BustTrade" | "Delivery")
}

/// `execFee` is carried with its sign. See the integration page for what the documentation states
/// and what it does not.
pub fn execution(
    row: &Value,
    scales: &InstrumentScales,
    received_at_ns: i128,
) -> Result<Execution, RowRefused> {
    check_category(row)?;
    let exec_type = text(row, "execType")?;
    if exec_type == "Funding" {
        return funding(row).map(Execution::Funding);
    }
    if !moves_position(exec_type) {
        return Err(RowRefused { field: "execType", detail: format!("{exec_type} is not mapped") });
    }
    let scales = scales_of(row, scales)?;
    let fee_scale = Scale::new(FEE_SCALE)
        .map_err(|e| RowRefused { field: "execFee", detail: format!("{e:?}") })?;
    let fee_text = text(row, "execFee")?;
    let fee = if fee_text.is_empty() {
        None
    } else {
        Some(Fee {
            asset: text(row, "feeCurrency").unwrap_or("").to_string(),
            amount_scaled: scaled(row, "execFee", fee_scale)?,
            scale: FEE_SCALE,
        })
    };
    let liquidity = match row.get("isMaker").and_then(Value::as_bool) {
        Some(true) => Liquidity::Maker,
        Some(false) => Liquidity::Taker,
        None => Liquidity::NotPublished,
    };
    Ok(Execution::Fill(Box::new(FillReport {
        client_id: client_id(row)?,
        instrument: VenueSymbol::new(text(row, "symbol")?),
        fill: Fill {
            fill_id: text(row, "execId")?.to_string(),
            venue_order_id: Some(text(row, "orderId")?.to_string()),
            side: side(row)?,
            price_scaled: scaled(row, "execPrice", scales.price)?,
            quantity_scaled: scaled(row, "execQty", scales.quantity)?,
            fee,
            liquidity,
            exchange_time: millis(row, "execTime")?,
            received_at_ns,
        },
    })))
}

/// A funding row as a charge, identified by its `execId`. `execFee` is positive when the account
/// paid: a long ETHUSDT position at a positive rate read `execFee` `0.00322492` while the
/// transaction log wrote `funding` `-0.00322492` for the same `tradeId` (2026-11-19 16:00 UTC).
/// A negative figure, funding received, is read the same way and is not yet seen on a record.
pub fn funding(row: &Value) -> Result<AccountCharge, RowRefused> {
    let fee_scale = Scale::new(FEE_SCALE)
        .map_err(|e| RowRefused { field: "execFee", detail: format!("{e:?}") })?;
    Ok(AccountCharge {
        charge_id: text(row, "execId")?.to_string(),
        fee: Fee {
            asset: text(row, "feeCurrency").unwrap_or("").to_string(),
            amount_scaled: scaled(row, "execFee", fee_scale)?,
            scale: FEE_SCALE,
        },
    })
}

/// The unified account's margin from its wallet-balance row (V5 Get Wallet Balance, read
/// 2026-11-20): `totalMarginBalance` ("totalWalletBalance + totalPerpUPL", in USD, counting only
/// collateral the account has switched on) as the equity, and `totalMaintenanceMargin` (USD).
/// `totalEquity` is not used: it values every coin, collateral or not. `None` when the totals are
/// empty: the venue states account-wide fields as not applicable to isolated margin, and an
/// account in isolated margin mode answered them empty (2026-11-20). Another account type is
/// refused.
pub fn account_margin(row: &Value) -> Result<Option<AccountMargin>, RowRefused> {
    let kind = text(row, "accountType")?;
    if kind != "UNIFIED" {
        return Err(RowRefused { field: "accountType", detail: format!("{kind} is not UNIFIED") });
    }
    if text(row, "totalMarginBalance")?.is_empty() {
        return Ok(None);
    }
    let scale = Scale::new(FEE_SCALE)
        .map_err(|e| RowRefused { field: "totalMarginBalance", detail: format!("{e:?}") })?;
    Ok(Some(AccountMargin {
        currency: "usd".to_string(),
        equity_scaled: scaled(row, "totalMarginBalance", scale)?,
        maintenance_scaled: scaled(row, "totalMaintenanceMargin", scale)?,
        scale: FEE_SCALE,
    }))
}

/// One-way mode only: a hedge-mode position index is refused, since one instrument would then
/// carry two positions this platform nets into one.
pub fn position_report(
    row: &Value,
    scales: &InstrumentScales,
) -> Result<PositionReport, RowRefused> {
    let scales = scales_of(row, scales)?;
    match row.get("positionIdx").and_then(Value::as_i64) {
        Some(0) => {}
        other => {
            return Err(RowRefused {
                field: "positionIdx",
                detail: format!("{other:?} is not one-way mode"),
            });
        }
    }
    let size = scaled(row, "size", scales.quantity)?;
    let quantity_scaled = match text(row, "side")? {
        "Buy" => size,
        "Sell" => -size,
        "" if size == 0 => 0,
        other => {
            return Err(RowRefused {
                field: "side",
                detail: format!("{other:?} with size {size}"),
            });
        }
    };
    Ok(PositionReport { instrument: VenueSymbol::new(text(row, "symbol")?), quantity_scaled })
}

/// One page of a list query: its rows and the cursor to the next page, absent on the last.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub rows: Vec<Value>,
    pub next_cursor: Option<String>,
}

/// A refused page names why. A response the venue refused (`retCode` not zero) is never read as
/// an empty list.
pub fn read_page(status: u16, body: &str) -> Result<Page, RowRefused> {
    if status == 0 {
        return Err(RowRefused { field: "status", detail: format!("no answer: {body}") });
    }
    if !(200..=299).contains(&status) {
        return Err(RowRefused { field: "status", detail: format!("HTTP {status}") });
    }
    let parsed: Value = serde_json::from_str(body)
        .map_err(|_| RowRefused { field: "body", detail: "not JSON".into() })?;
    match parsed.get("retCode").and_then(Value::as_i64) {
        Some(0) => {}
        code => {
            let message = parsed.get("retMsg").and_then(Value::as_str).unwrap_or("");
            return Err(RowRefused { field: "retCode", detail: format!("{code:?}: {message}") });
        }
    }
    let rows = parsed
        .pointer("/result/list")
        .and_then(Value::as_array)
        .cloned()
        .ok_or(RowRefused { field: "result.list", detail: "absent".into() })?;
    let next_cursor = parsed
        .pointer("/result/nextPageCursor")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    Ok(Page { rows, next_cursor })
}

/// Pages a list query to its end. Bounded: a venue that keeps returning a cursor past the bound
/// yields an incomplete read, reported, not an endless one.
pub const MAX_PAGES: usize = 200;

/// The query for the next page. The cursor is the venue's opaque token, passed back unchanged.
pub fn page_query(base: &str, cursor: Option<&str>) -> String {
    match cursor {
        Some(c) => format!("{base}&cursor={c}"),
        None => base.to_string(),
    }
}

/// The rows one evidence read gathered, before they are judged.
#[derive(Debug, Clone, Default)]
pub struct Gathered {
    pub orders: Vec<Value>,
    pub executions: Vec<Value>,
    pub positions: Vec<Value>,
}

/// Judges every gathered row. A symbol of the catalogue absent from the position rows is an
/// explicit flat: queried by settle coin, the venue returns only positions of size greater than
/// zero (V5 Get Position Info, `symbol` parameter). Every refusal is returned beside the evidence,
/// which is then incomplete.
pub fn assemble(
    gathered: &Gathered,
    catalogue: &[VenueSymbol],
    scales: &InstrumentScales,
    since_ns: i128,
    received_at_ns: i128,
) -> (VenueEvidence, Vec<RowRefused>) {
    let mut refused = Vec::new();
    let mut orders: Vec<OrderStatusReport> = Vec::new();
    for row in &gathered.orders {
        match order_report(row, scales) {
            Ok(report) => keep_latest(&mut orders, report),
            Err(e) => refused.push(e),
        }
    }
    let mut fills = Vec::new();
    let mut charges = Vec::new();
    for row in &gathered.executions {
        match execution(row, scales, received_at_ns) {
            Ok(Execution::Fill(report)) => fills.push(*report),
            Ok(Execution::Funding(charge)) => charges.push(charge),
            Err(e) => refused.push(e),
        }
    }
    let mut positions: Vec<PositionReport> = Vec::new();
    for row in &gathered.positions {
        match position_report(row, scales) {
            Ok(report) => positions.push(report),
            Err(e) => refused.push(e),
        }
    }
    for symbol in catalogue {
        if !positions.iter().any(|p| &p.instrument == symbol) {
            positions.push(PositionReport { instrument: symbol.clone(), quantity_scaled: 0 });
        }
    }
    let evidence = VenueEvidence {
        charges,
        orders,
        fills,
        positions: PositionEvidence::Reported(positions),
        history_from_ns: Some(since_ns),
        complete: refused.is_empty(),
    };
    (evidence, refused)
}

/// Adds each pushed fill the history read did not list. The pushed row is the same record as
/// the listed one (identity, figures, fee and stamp agreed on every pair compared 2026-11-18), so
/// a fill listed later is the one already applied. A pushed fill executed before the window is
/// left out, as the history would leave it.
pub fn with_pushed(evidence: &mut VenueEvidence, pushed: &[FillReport], since_ns: i128) {
    for report in pushed {
        let listed = evidence.fills.iter().any(|f| f.fill.fill_id == report.fill.fill_id);
        let inside = match report.fill.exchange_time {
            ExchangeTime::At { at_ns } => at_ns >= since_ns,
            ExchangeTime::NotPublished => true,
        };
        if !listed && inside {
            evidence.fills.push(report.clone());
        }
    }
}

/// An order listed by both the open and the recent-closed query keeps its latest state.
/// Whether a closed order from the recent list changed at or after `since_ms`. One that closed
/// before the window moved nothing the window covers. Whatever it left is judged on the position.
/// An order that states no readable time is kept, so a changed format is judged, not dropped.
#[must_use]
pub fn changed_since(row: &Value, since_ms: i128) -> bool {
    row.get("updatedTime")
        .and_then(Value::as_str)
        .and_then(|at| at.parse::<i128>().ok())
        .is_none_or(|at| at >= since_ms)
}

fn keep_latest(orders: &mut Vec<OrderStatusReport>, report: OrderStatusReport) {
    let later = |a: &ExchangeTime, b: &ExchangeTime| match (a, b) {
        (ExchangeTime::At { at_ns: x }, ExchangeTime::At { at_ns: y }) => x >= y,
        _ => true,
    };
    match orders.iter_mut().find(|o| o.venue_order_id == report.venue_order_id) {
        Some(held) if later(&report.reported_at, &held.reported_at) => *held = report,
        Some(_) => {}
        None => orders.push(report),
    }
}

#[cfg(test)]
#[path = "../tests/unit/evidence_tests.rs"]
mod tests;
