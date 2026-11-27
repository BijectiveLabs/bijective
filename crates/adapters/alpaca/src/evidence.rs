// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue evidence read from the venue's own records: its orders, its fill activities and its
//! positions. Pure over the answers, so documented bodies are checked offline.
//!
//! The venue carries the platform's client identifier on every order, so an order is linked by
//! it. A fill names only the venue's order identifier and is linked through the orders read. A
//! row that cannot be judged is refused with the field that failed and marks the evidence
//! incomplete. It is never dropped. Fields and open questions: `docs/integrations/alpaca.md`.

use std::collections::BTreeMap;

use bijective_lifecycle::{
    AccountCharge, Fee, Fill, FillReport, Liquidity, OrderStatusReport, PositionReport,
    ReportedStatus,
};
use bijective_model::market::ExchangeTime;
use bijective_model::{AccountMargin, PositionSnapshot, Side, VenueSymbol};
use bijective_venue::normalise::{Scale, parse_scaled};
use serde_json::Value;

/// The platform's client identifier for each venue order identifier read.
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
    match text(row, "side")? {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        other => Err(refused("side", format!("{other} is neither buy nor sell"))),
    }
}

/// Days from 1970-01-01 to a civil date in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn digits(text: &str) -> Option<i64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok())?
}

/// An RFC 3339 instant in UTC nanoseconds: `2022-02-02T18:55:09.981482Z`, with up to nine
/// fractional digits, or with an offset such as `-04:00` (both are in the venue's examples).
#[must_use]
pub fn utc_ns(stamp: &str) -> Option<i128> {
    let (date, rest) = stamp.split_once('T')?;
    let mut date = date.splitn(3, '-');
    let (year, month, day) = (digits(date.next()?)?, digits(date.next()?)?, digits(date.next()?)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset_s) = if let Some(clock) = rest.strip_suffix('Z') {
        (clock, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (clock, offset) = rest.split_at(at);
        let (hours, minutes) = offset[1..].split_once(':')?;
        let magnitude = digits(hours)? * 3600 + digits(minutes)? * 60;
        (clock, if offset.starts_with('-') { -magnitude } else { magnitude })
    };
    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = whole.splitn(3, ':');
    let (hour, minute, second) =
        (digits(parts.next()?)?, digits(parts.next()?)?, digits(parts.next()?)?);
    if hour > 23 || minute > 59 || second > 60 || fraction.len() > 9 {
        return None;
    }
    let nanos = if fraction.is_empty() { 0 } else { digits(fraction)? };
    let nanos = i128::from(nanos) * 10_i128.pow(9 - fraction.len() as u32);
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset_s;
    Some(i128::from(seconds) * 1_000_000_000 + nanos)
}

/// Whole seconds as the activities' `after` takes them, `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
pub fn utc_text(at_ns: i128) -> String {
    let seconds = i64::try_from(at_ns.div_euclid(1_000_000_000)).unwrap_or(0);
    let (days, of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

fn stamp_of(row: &Value, field: &'static str) -> ExchangeTime {
    match row.get(field).and_then(Value::as_str).and_then(utc_ns) {
        Some(at_ns) => ExchangeTime::At { at_ns },
        None => ExchangeTime::NotPublished,
    }
}

/// The statuses the reference lists (OrderStatus). `held`, whose meaning is not stated,
/// `replaced` and `calculated` are refused for an operator rather than guessed.
fn order_status(status: &str) -> Result<ReportedStatus, RowRefused> {
    match status {
        "new"
        | "accepted"
        | "pending_new"
        | "accepted_for_bidding"
        | "partially_filled"
        | "pending_cancel"
        | "pending_replace"
        | "done_for_day"
        | "stopped"
        | "suspended" => Ok(ReportedStatus::Working),
        "filled" => Ok(ReportedStatus::Filled),
        "canceled" => Ok(ReportedStatus::Canceled),
        "expired" => Ok(ReportedStatus::Expired),
        "rejected" => Ok(ReportedStatus::Rejected),
        other => Err(refused("status", format!("{other} is not a status this build reads"))),
    }
}

/// One order object. The client identifier is the venue's `client_order_id`. One the platform
/// did not write reads as external downstream.
pub fn order_report(row: &Value, scales: Scales) -> Result<OrderStatusReport, RowRefused> {
    Ok(OrderStatusReport {
        client_id: row
            .get("client_order_id")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string),
        venue_order_id: text(row, "id")?.to_string(),
        instrument: VenueSymbol::new(text(row, "symbol")?),
        side: side(row)?,
        ordered_quantity_scaled: scaled(row, "qty", scales.quantity)?,
        filled_quantity_scaled: scaled(row, "filled_qty", scales.quantity)?,
        status: order_status(text(row, "status")?)?,
        reported_at: stamp_of(row, "updated_at"),
    })
}

/// One `FILL` activity. Its identifier is the venue's, unique per execution, so a fill read twice
/// is recognised. The activity states no fee: fees are separate `FEE` activities.
pub fn fill_report(
    row: &Value,
    clients: &ClientOf,
    scales: Scales,
    received_at_ns: i128,
) -> Result<FillReport, RowRefused> {
    let order_id = text(row, "order_id")?;
    Ok(FillReport {
        client_id: clients.get(order_id).cloned(),
        instrument: VenueSymbol::new(text(row, "symbol")?),
        fill: Fill {
            fill_id: text(row, "id")?.to_string(),
            venue_order_id: Some(order_id.to_string()),
            side: side(row)?,
            price_scaled: scaled(row, "price", scales.price)?,
            quantity_scaled: scaled(row, "qty", scales.quantity)?,
            fee: None,
            liquidity: Liquidity::NotPublished,
            exchange_time: stamp_of(row, "transaction_time"),
            received_at_ns,
        },
    })
}

/// Eight decimals: a fee is charged in cents, and a figure finer than this is refused, not cut.
const CHARGE_SCALE: u8 = 8;

/// One `FEE` activity, as a charge the account paid. The reference documents `net_amount` only as
/// positive or negative. The account's activities state money in as positive (a deposit read
/// 2026-11-18), so a fee paid is negative there and positive here, as the account's own fees showed
/// (2026-11-19: CAT and REG fees of -0.01 to -0.03). A missing `currency` leaves the
/// asset empty, which the daily loss counts as not valued rather than guessing it.
pub fn charge_report(row: &Value) -> Result<AccountCharge, RowRefused> {
    let scale = Scale::new(CHARGE_SCALE).map_err(|e| refused("net_amount", format!("{e:?}")))?;
    let net = scaled(row, "net_amount", scale)?;
    let asset = row.get("currency").and_then(Value::as_str).unwrap_or_default();
    Ok(AccountCharge {
        charge_id: text(row, "id")?.to_string(),
        fee: Fee {
            asset: asset.to_string(),
            amount_scaled: net.checked_neg().ok_or_else(|| refused("net_amount", "overflow"))?,
            scale: CHARGE_SCALE,
        },
    })
}

/// The account's margin (Get account, read 2026-11-20): `equity` ("Cash + long_market_value +
/// short_market_value") and `maintenance_margin` ("Maintenance margin requirement (continuously
/// updated value)"), in the account's `currency`. The venue assesses margin calls at the end of the
/// day. The figure here is the one it updates continuously.
pub fn account_margin(account: &Value) -> Result<AccountMargin, RowRefused> {
    let scale = Scale::new(CHARGE_SCALE).map_err(|e| refused("equity", format!("{e:?}")))?;
    let currency = text(account, "currency")?;
    if currency.is_empty() {
        return Err(refused("currency", "empty"));
    }
    Ok(AccountMargin {
        currency: currency.to_ascii_lowercase(),
        equity_scaled: scaled(account, "equity", scale)?,
        maintenance_scaled: scaled(account, "maintenance_margin", scale)?,
        scale: CHARGE_SCALE,
    })
}

/// One position, signed by its `side`: the reference states `long` or `short` and does not say
/// whether a short's `qty` carries a sign, so its magnitude is taken and the side decides.
pub fn position_report(row: &Value, scales: Scales) -> Result<PositionSnapshot, RowRefused> {
    let magnitude = scaled(row, "qty", scales.quantity)?.abs();
    let quantity_scaled = match text(row, "side")? {
        "long" => magnitude,
        "short" => -magnitude,
        other => return Err(refused("side", format!("{other} is neither long nor short"))),
    };
    Ok(PositionSnapshot {
        instrument: VenueSymbol::new(text(row, "symbol")?),
        quantity_scaled,
        entry_price_scaled: scaled(row, "avg_entry_price", scales.price)?,
    })
}

/// The position as reconciliation compares it.
#[must_use]
pub fn reported(position: &PositionSnapshot) -> PositionReport {
    PositionReport {
        instrument: position.instrument.clone(),
        quantity_scaled: position.quantity_scaled,
    }
}
