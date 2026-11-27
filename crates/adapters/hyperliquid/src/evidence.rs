// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue evidence from the account's own records: orders, fills, funding and positions, judged
//! row by row. A row that cannot be judged is refused with the field that failed and marks the
//! evidence incomplete. It is never dropped. The venue knows an order by a 128-bit `cloid` this
//! platform derives from its own identifier, so `Clients` maps it back. Field meanings and what
//! was read from live answers: `docs/integrations/hyperliquid.md`, "Venue evidence".

use std::collections::BTreeMap;

use bijective_lifecycle::{
    AccountCharge, Fee, Fill, FillReport, Liquidity, OrderStatusReport, ReportedStatus,
};
use bijective_model::market::ExchangeTime;
use bijective_venue::contract::{Recorded, Side, VenueSymbol};
use bijective_venue::normalise::{Scale, parse_scaled};
use serde_json::Value;

use crate::rest::cloid_for;

/// Fees and funding arrive with six places. Twelve holds them exactly and anything finer is
/// refused rather than rounded.
pub const FEE_SCALE: u8 = 12;

/// The asset funding is paid in: the venue names the amount `usdc`.
pub const FUNDING_ASSET: &str = "USDC";

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

/// The platform's orders on this venue by cloid, and whether each may still be live.
#[derive(Debug, Clone, Default)]
pub struct Clients {
    by_cloid: BTreeMap<String, (String, bool)>,
}

impl Clients {
    /// Called before the submit is sent, so an order the venue took is always known.
    pub fn placed(&mut self, client_id: &str) {
        self.by_cloid.entry(cloid_for(client_id)).or_insert((client_id.to_string(), true));
    }

    /// Orders a previous process placed, from the record.
    pub fn adopt(&mut self, recorded: &[Recorded]) {
        for order in recorded {
            let client_id = order.request.client_id.clone();
            self.by_cloid.entry(cloid_for(&client_id)).or_insert((client_id, !order.terminal));
        }
    }

    /// The platform's identifier behind `cloid`. `None` for an order this platform did not place.
    #[must_use]
    pub fn client_of(&self, cloid: Option<&str>) -> Option<String> {
        let cloid = cloid?.to_ascii_lowercase();
        self.by_cloid.get(&cloid).map(|(client_id, _)| client_id.clone())
    }

    /// The cloids of orders that may still be live, so one the history list no longer holds
    /// is asked for by name.
    #[must_use]
    pub fn unsettled(&self) -> Vec<String> {
        self.by_cloid.iter().filter(|(_, (_, open))| *open).map(|(c, _)| c.clone()).collect()
    }

    /// An order the venue reports finished is not asked for again.
    pub fn settle(&mut self, orders: &[OrderStatusReport]) {
        for order in orders.iter().filter(|o| o.status != ReportedStatus::Working) {
            let Some(client_id) = &order.client_id else { continue };
            if let Some(entry) = self.by_cloid.get_mut(&cloid_for(client_id)) {
                entry.1 = false;
            }
        }
    }
}

fn text<'a>(row: &'a Value, field: &'static str) -> Result<&'a str, RowRefused> {
    row.get(field).and_then(Value::as_str).ok_or_else(|| refused(field, "absent or not text"))
}

fn scaled(row: &Value, field: &'static str, scale: Scale) -> Result<i128, RowRefused> {
    let raw = text(row, field)?;
    parse_scaled(raw, scale).map_err(|e| refused(field, format!("{raw}: {e:?}")))
}

fn unsigned(row: &Value, field: &'static str) -> Result<u64, RowRefused> {
    row.get(field).and_then(Value::as_u64).ok_or_else(|| refused(field, "absent or not a count"))
}

fn side(row: &Value) -> Result<Side, RowRefused> {
    match text(row, "side")? {
        "B" => Ok(Side::Buy),
        "A" => Ok(Side::Sell),
        other => Err(refused("side", format!("{other} is not B or A"))),
    }
}

fn fee_scale() -> Result<Scale, RowRefused> {
    Scale::new(FEE_SCALE).map_err(|e| refused("fee", format!("{e:?}")))
}

/// Every status the order status table documents (read 2026-11-23). `triggered` is left
/// unmapped: this adapter places no trigger orders, and what a triggered row means for the
/// order it became is not stated.
fn status(raw: &str) -> Result<ReportedStatus, RowRefused> {
    const CANCELED: &[&str] = &[
        "canceled",
        "marginCanceled",
        "vaultWithdrawalCanceled",
        "openInterestCapCanceled",
        "selfTradeCanceled",
        "reduceOnlyCanceled",
        "siblingFilledCanceled",
        "delistedCanceled",
        "liquidatedCanceled",
        "scheduledCancel",
    ];
    const REJECTED: &[&str] = &[
        "rejected",
        "tickRejected",
        "minTradeNtlRejected",
        "perpMarginRejected",
        "reduceOnlyRejected",
        "badAloPxRejected",
        "iocCancelRejected",
        "badTriggerPxRejected",
        "marketOrderNoLiquidityRejected",
        "positionIncreaseAtOpenInterestCapRejected",
        "positionFlipAtOpenInterestCapRejected",
        "tooAggressiveAtOpenInterestCapRejected",
        "openInterestIncreaseRejected",
        "insufficientSpotBalanceRejected",
        "oracleRejected",
        "perpMaxPositionRejected",
    ];
    match raw {
        "open" => Ok(ReportedStatus::Working),
        "filled" => Ok(ReportedStatus::Filled),
        s if CANCELED.contains(&s) => Ok(ReportedStatus::Canceled),
        s if REJECTED.contains(&s) => Ok(ReportedStatus::Rejected),
        other => Err(refused("status", format!("{other} is not mapped"))),
    }
}

/// One entry of `historicalOrders`, or the `order` of an `orderStatus` answer: `{order, status,
/// statusTimestamp}`. `sz` is what remains, so the filled quantity is `origSz - sz`.
pub fn order_report(
    entry: &Value,
    clients: &Clients,
    quantity: Scale,
) -> Result<OrderStatusReport, RowRefused> {
    let order = entry.get("order").ok_or_else(|| refused("order", "absent"))?;
    let ordered = scaled(order, "origSz", quantity)?;
    let remaining = scaled(order, "sz", quantity)?;
    Ok(OrderStatusReport {
        client_id: clients.client_of(order.get("cloid").and_then(Value::as_str)),
        venue_order_id: unsigned(order, "oid")?.to_string(),
        instrument: VenueSymbol::new(text(order, "coin")?),
        side: side(order)?,
        ordered_quantity_scaled: ordered,
        filled_quantity_scaled: ordered - remaining,
        status: status(text(entry, "status")?)?,
        reported_at: ExchangeTime::from_ms(unsigned(entry, "statusTimestamp")?),
    })
}

/// One fill of `userFillsByTime`. `fee` includes any builder fee and is negative for a rebate.
/// `crossed` is true for the side that took liquidity.
pub fn fill_report(
    row: &Value,
    clients: &Clients,
    price: Scale,
    quantity: Scale,
    received_at_ns: i128,
) -> Result<FillReport, RowRefused> {
    let liquidity = match row.get("crossed").and_then(Value::as_bool) {
        Some(true) => Liquidity::Taker,
        Some(false) => Liquidity::Maker,
        None => Liquidity::NotPublished,
    };
    let fee = Fee {
        asset: text(row, "feeToken")?.trim().to_string(),
        amount_scaled: scaled(row, "fee", fee_scale()?)?,
        scale: FEE_SCALE,
    };
    Ok(FillReport {
        client_id: clients.client_of(row.get("cloid").and_then(Value::as_str)),
        instrument: VenueSymbol::new(text(row, "coin")?),
        fill: Fill {
            fill_id: unsigned(row, "tid")?.to_string(),
            venue_order_id: Some(unsigned(row, "oid")?.to_string()),
            side: side(row)?,
            price_scaled: scaled(row, "px", price)?,
            quantity_scaled: scaled(row, "sz", quantity)?,
            fee: Some(fee),
            liquidity,
            exchange_time: ExchangeTime::from_ms(unsigned(row, "time")?),
            received_at_ns,
        },
    })
}

/// One row of `userFunding`. `usdc` is the account's cash change, so the charge, positive when
/// paid, is its negation. The venue stamps funding with an all-zero hash. One coin is charged
/// once per stamp, so coin and stamp identify it.
pub fn funding_charge(row: &Value) -> Result<AccountCharge, RowRefused> {
    let delta = row.get("delta").ok_or_else(|| refused("delta", "absent"))?;
    if text(delta, "type")? != "funding" {
        return Err(refused("type", "not a funding row"));
    }
    let coin = text(delta, "coin")?;
    let time = unsigned(row, "time")?;
    let cash = scaled(delta, "usdc", fee_scale()?)?;
    Ok(AccountCharge {
        charge_id: format!("funding:{coin}:{time}"),
        fee: Fee { asset: FUNDING_ASSET.to_string(), amount_scaled: -cash, scale: FEE_SCALE },
    })
}

/// The venue lists a row per change of an order, newest first: the latest state is kept, and at
/// one stamp a finished state outranks `open`.
pub fn keep_latest(orders: &mut Vec<OrderStatusReport>, report: OrderStatusReport) {
    let rank = |o: &OrderStatusReport| {
        let at = match o.reported_at {
            ExchangeTime::At { at_ns } => at_ns,
            ExchangeTime::NotPublished => i128::MIN,
        };
        (at, o.status != ReportedStatus::Working)
    };
    match orders.iter_mut().find(|o| o.venue_order_id == report.venue_order_id) {
        Some(held) if rank(&report) >= rank(held) => *held = report,
        Some(_) => {}
        None => orders.push(report),
    }
}

#[cfg(test)]
#[path = "../tests/unit/evidence_tests.rs"]
mod tests;
