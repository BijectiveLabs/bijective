// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The open format of the `performance` stream: what one strategy's orders on one instrument
//! came to, as a program that read the record concluded, one JSON object per entry. See
//! `docs/concepts/observability.md`, "Strategy performance".
//!
//! The console reads the stream and writes nothing to it. `encode` is here so that a writer,
//! the platform's or a third party's, produces exactly what `decode` accepts.

use serde_json::{Map, Value, json};

/// The stream the console reads strategy performance from.
pub const PERFORMANCE: &str = "performance";

/// The performance of the node of `mode`: `performance` for live, `performance-<mode>` for the
/// others, kept apart like the node's own streams.
#[must_use]
pub fn stream_of(mode: &str) -> String {
    if mode == "live" { PERFORMANCE.to_string() } else { format!("{PERFORMANCE}-{mode}") }
}

/// Each run mode and its performance stream, live first.
#[must_use]
pub fn every_mode() -> Vec<(String, String)> {
    use bijective_serialization::signal::node::{BACKTEST, LIVE, PAPER};
    [LIVE, PAPER, BACKTEST].iter().map(|n| (n.mode.to_string(), stream_of(n.mode))).collect()
}

/// The format's version, in every entry. A change that breaks a reader raises it.
pub const PERFORMANCE_FORMAT: u64 = 1;

/// The horizons the console shows a markout at, in seconds. A writer measures at these.
pub const MARKOUT_HORIZONS_S: [u64; 5] = [1, 5, 10, 30, 60];

/// How far the market moved from a fill's price, a horizon after the fill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Markout {
    pub horizon_s: u64,
    /// Basis points, notional-weighted over the fills measured. Positive is in the fill's favour.
    pub bps: Option<String>,
    /// The fills it is measured over: one the market had not yet reached is not among them.
    pub fills: u64,
}

/// One subject: a desk's strategy on one instrument of one venue. Money is an exact decimal in
/// the instrument's quote currency. A figure that cannot be made is `None`, never zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Performance {
    pub desk: String,
    pub strategy: Option<String>,
    pub venue: String,
    /// The venue's symbol, as the orders name it.
    pub instrument: String,
    pub identity: Option<String>,
    pub quote_currency: Option<String>,
    pub fills: u64,
    /// Closing fills that realized a gain, and a loss.
    pub wins: u64,
    pub losses: u64,
    pub position: String,
    pub traded_notional: String,
    pub realized: String,
    pub unrealized: Option<String>,
    /// Fees charged in the quote currency. Fees in another asset are named in `fees_excluded`.
    pub fees: String,
    pub fees_excluded: Vec<String>,
    /// Realized plus unrealized less fees.
    pub net: Option<String>,
    pub peak_net: Option<String>,
    pub drawdown: Option<String>,
    pub arrival_cost: Option<String>,
    pub arrival_cost_bps: Option<String>,
    pub markouts: Vec<Markout>,
    /// The control stream folded, and the last position of it this entry reflects.
    pub source: String,
    pub through: u64,
    /// The record's time this entry was concluded at, UTC nanoseconds.
    pub at_ns: i128,
}

/// Why an entry is not a performance record. The console lists it as unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused(pub &'static str);

/// The entry as one line of JSON. Integers as decimal strings, like the console's tables.
#[must_use]
pub fn encode(p: &Performance) -> Vec<u8> {
    let markouts: Vec<Value> = p
        .markouts
        .iter()
        .map(|m| json!({ "horizon_s": m.horizon_s, "bps": m.bps, "fills": m.fills }))
        .collect();
    let value = json!({
        "format": PERFORMANCE_FORMAT,
        "desk": p.desk, "strategy": p.strategy, "venue": p.venue, "instrument": p.instrument,
        "identity": p.identity, "quote_currency": p.quote_currency,
        "fills": p.fills, "wins": p.wins, "losses": p.losses,
        "position": p.position, "traded_notional": p.traded_notional,
        "realized": p.realized, "unrealized": p.unrealized,
        "fees": p.fees, "fees_excluded": p.fees_excluded,
        "net": p.net, "peak_net": p.peak_net, "drawdown": p.drawdown,
        "arrival_cost": p.arrival_cost, "arrival_cost_bps": p.arrival_cost_bps,
        "markouts": markouts,
        "source": p.source, "through": p.through.to_string(), "at_ns": p.at_ns.to_string(),
    });
    value.to_string().into_bytes()
}

/// Accepts exactly what `encode` writes: every field present, names and figures well formed.
///
/// # Errors
/// `Refused` naming the first field that is missing or malformed.
pub fn decode(payload: &[u8]) -> Result<Performance, Refused> {
    let value: Value = serde_json::from_slice(payload).map_err(|_| Refused("not JSON"))?;
    let o = value.as_object().ok_or(Refused("not an object"))?;
    if o.get("format").and_then(Value::as_u64) != Some(PERFORMANCE_FORMAT) {
        return Err(Refused("format"));
    }
    Ok(Performance {
        desk: named(o, "desk")?,
        strategy: optional_text(o, "strategy")?,
        venue: named(o, "venue")?,
        instrument: named(o, "instrument")?,
        identity: optional_text(o, "identity")?,
        quote_currency: optional_text(o, "quote_currency")?,
        fills: count(o, "fills")?,
        wins: count(o, "wins")?,
        losses: count(o, "losses")?,
        position: required(o, "position")?,
        traded_notional: required(o, "traded_notional")?,
        realized: required(o, "realized")?,
        unrealized: figure(o, "unrealized")?,
        fees: required(o, "fees")?,
        fees_excluded: texts(o, "fees_excluded")?,
        net: figure(o, "net")?,
        peak_net: figure(o, "peak_net")?,
        drawdown: figure(o, "drawdown")?,
        arrival_cost: figure(o, "arrival_cost")?,
        arrival_cost_bps: figure(o, "arrival_cost_bps")?,
        markouts: markouts(o)?,
        source: named(o, "source")?,
        through: digits(o, "through")?,
        at_ns: text(o, "at_ns")?.parse::<i128>().map_err(|_| Refused("at_ns"))?,
    })
}

fn markouts(o: &Map<String, Value>) -> Result<Vec<Markout>, Refused> {
    let listed = o.get("markouts").and_then(Value::as_array).ok_or(Refused("markouts"))?;
    listed
        .iter()
        .map(|m| {
            let m = m.as_object().ok_or(Refused("markouts"))?;
            Ok(Markout {
                horizon_s: count(m, "horizon_s")?,
                bps: figure(m, "bps")?,
                fills: count(m, "fills")?,
            })
        })
        .collect()
}

fn text(o: &Map<String, Value>, field: &'static str) -> Result<String, Refused> {
    o.get(field).and_then(Value::as_str).map(str::to_string).ok_or(Refused(field))
}

fn named(o: &Map<String, Value>, field: &'static str) -> Result<String, Refused> {
    text(o, field).and_then(|t| if t.is_empty() { Err(Refused(field)) } else { Ok(t) })
}

fn optional_text(o: &Map<String, Value>, field: &'static str) -> Result<Option<String>, Refused> {
    match o.get(field) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) if !raw.is_empty() => Ok(Some(raw.clone())),
        _ => Err(Refused(field)),
    }
}

fn texts(o: &Map<String, Value>, field: &'static str) -> Result<Vec<String>, Refused> {
    let listed = o.get(field).and_then(Value::as_array).ok_or(Refused(field))?;
    listed.iter().map(|v| v.as_str().map(str::to_string).ok_or(Refused(field))).collect()
}

fn count(o: &Map<String, Value>, field: &'static str) -> Result<u64, Refused> {
    o.get(field).and_then(Value::as_u64).ok_or(Refused(field))
}

fn digits(o: &Map<String, Value>, field: &'static str) -> Result<u64, Refused> {
    text(o, field)?.parse::<u64>().map_err(|_| Refused(field))
}

fn required(o: &Map<String, Value>, field: &'static str) -> Result<String, Refused> {
    figure(o, field)?.ok_or(Refused(field))
}

/// An exact decimal written out, or `null` where none could be made.
fn figure(o: &Map<String, Value>, field: &'static str) -> Result<Option<String>, Refused> {
    match o.get(field) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) if crate::detection::is_decimal(raw) => Ok(Some(raw.clone())),
        _ => Err(Refused(field)),
    }
}

#[cfg(test)]
#[path = "../tests/unit/performance_tests.rs"]
mod tests;
