// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What each order's fills cost against the mid of the book its strategy decided on (`Captured`),
//! from the record alone (`docs/concepts/execution_quality.md`). Positive is a cost: crossing is
//! mid to touch, what immediacy costs, and residual is touch to fill. Fees are left to the fills
//! table, in the asset charged. Every cost is exact in the quote currency. The average price and
//! the basis points are rounded half away from zero at the scale their column states.

use std::collections::BTreeMap;

use bijective_control_engine::wire::{CapturedBook, ControlEvent};
use bijective_model::Side;

use super::keep;
use super::scales::Scales;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Panel};
use crate::table::{Cell, Table, Unit};

const COLUMNS: &[(&str, Unit)] = &[
    ("decided at", Unit::Nanoseconds),
    ("venue", Unit::Text),
    ("instrument", Unit::Text),
    ("decision", Unit::Text),
    ("client id", Unit::Text),
    ("side", Unit::Text),
    ("bid at decision", Unit::Decimal),
    ("ask at decision", Unit::Decimal),
    ("book age at decision", Unit::Span),
    ("no benchmark because", Unit::Text),
    ("fills", Unit::Count),
    ("filled quantity", Unit::Decimal),
    ("average price", Unit::Decimal),
    ("arrival cost", Unit::Decimal),
    ("arrival cost bps", Unit::Decimal),
    ("crossing cost", Unit::Decimal),
    ("residual cost", Unit::Decimal),
    ("decision to first fill", Unit::Span),
];

/// Decimals added to the price scale for the average price.
const AVERAGE_EXTRA_DIGITS: u8 = 4;
/// Basis points are shown to the hundredth.
pub const BPS_SCALE: u8 = 2;

/// What the strategy saw when it decided.
#[derive(Debug, Clone)]
struct Benchmark {
    decided_at_ns: i128,
    book: Option<CapturedBook>,
    detail: String,
}

/// One order and what its fills added up to.
#[derive(Debug, Clone)]
pub struct CostRow {
    pub decision: String,
    pub client_id: String,
    pub venue: Option<String>,
    pub instrument: String,
    pub side: Side,
    pub fills: usize,
    pub quantity_scaled: i128,
    /// Sum of price times quantity, at the price scale plus the quantity scale.
    pub notional_scaled: i128,
    pub first_fill_ns: Option<i128>,
}

#[derive(Default)]
pub struct ExecutionPanel {
    pub orders: Vec<CostRow>,
    benchmarks: BTreeMap<String, Benchmark>,
    scales: Scales,
}

/// The arrival cost's parts, exact at the price scale plus the quantity scale plus one (the mid
/// is half a tick finer than the book).
pub struct Costs {
    pub arrival: i128,
    pub crossing: i128,
    /// At [`BPS_SCALE`].
    pub bps_scaled: i128,
}

/// The arrival cost of `quantity` filled for `notional` (price times quantity, at the price
/// scale plus the quantity scale) against `book`. `None` without a positive mid or on overflow.
#[must_use]
pub fn costs(side: Side, book: &CapturedBook, quantity: i128, notional: i128) -> Option<Costs> {
    let sign = match side {
        Side::Buy => 1,
        Side::Sell => -1,
    };
    let (bid, ask) = (i128::from(book.best_bid_ticks), i128::from(book.best_ask_ticks));
    let twice_mid = bid.checked_add(ask)?;
    let touch = match side {
        Side::Buy => ask,
        Side::Sell => bid,
    };
    // Twice each figure at the combined scale is five times it at one decimal more.
    let twice_arrival =
        sign * notional.checked_mul(2)?.checked_sub(twice_mid.checked_mul(quantity)?)?;
    let twice_crossing =
        sign * touch.checked_mul(2)?.checked_sub(twice_mid)?.checked_mul(quantity)?;
    let at_mid = twice_mid.checked_mul(quantity)?;
    if at_mid <= 0 {
        return None;
    }
    let per_bps = 10_000 * 10i128.pow(u32::from(BPS_SCALE));
    let bps_scaled = rounded(twice_arrival.checked_mul(per_bps)?, at_mid);
    Some(Costs {
        arrival: twice_arrival.checked_mul(5)?,
        crossing: twice_crossing.checked_mul(5)?,
        bps_scaled,
    })
}

/// `value / by`, rounded half away from zero. `by` is positive.
fn rounded(value: i128, by: i128) -> i128 {
    let (quotient, remainder) = (value / by, value % by);
    if remainder.abs() * 2 >= by { quotient + value.signum() } else { quotient }
}

impl ExecutionPanel {
    fn cost_cells(&self, r: &CostRow, benchmark: Option<&Benchmark>) -> [Cell; 5] {
        let scales = self.scales.of(r.venue.as_deref(), &r.instrument);
        // Notional over quantity is at the price scale. The extra digits are kept, then rounded.
        let average = scales.filter(|_| r.quantity_scaled > 0).map(|(price, _)| {
            let extra = 10i128.pow(u32::from(AVERAGE_EXTRA_DIGITS));
            let average = rounded(r.notional_scaled.saturating_mul(extra), r.quantity_scaled);
            Cell::decimal(Some(average), Some(price + AVERAGE_EXTRA_DIGITS))
        });
        let book = benchmark.and_then(|b| b.book.as_ref());
        let parts = book
            .filter(|_| r.quantity_scaled > 0)
            .and_then(|book| costs(r.side, book, r.quantity_scaled, r.notional_scaled));
        let combined = scales.map(|(price, quantity)| price + quantity + 1);
        let at = |v: Option<i128>| Cell::decimal(v, combined);
        [
            average.unwrap_or(Cell::Absent),
            at(parts.as_ref().map(|c| c.arrival)),
            Cell::decimal(parts.as_ref().map(|c| c.bps_scaled), Some(BPS_SCALE)),
            at(parts.as_ref().map(|c| c.crossing)),
            at(parts.as_ref().map(|c| c.arrival - c.crossing)),
        ]
    }

    fn add_fill(&mut self, client_id: &str, price: i128, quantity: i128, received_ns: i128) {
        let Some(row) = self.orders.iter_mut().rev().find(|r| r.client_id == client_id) else {
            return;
        };
        row.fills += 1;
        row.quantity_scaled = row.quantity_scaled.saturating_add(quantity);
        row.notional_scaled = row.notional_scaled.saturating_add(price.saturating_mul(quantity));
        row.first_fill_ns = Some(row.first_fill_ns.map_or(received_ns, |f| f.min(received_ns)));
    }
}

impl Panel for ExecutionPanel {
    fn title(&self) -> &'static str {
        "Execution cost"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(event) = entry else { return };
        self.scales.learn(event);
        match event {
            ControlEvent::Captured { correlation_id, decided_at_ns, book, detail, .. } => {
                let benchmark = Benchmark {
                    decided_at_ns: *decided_at_ns,
                    book: *book,
                    detail: detail.clone(),
                };
                self.benchmarks.insert(correlation_id.clone(), benchmark);
            }
            ControlEvent::Intent { decision_id, venue, request, .. } => {
                let row = CostRow {
                    decision: decision_id.clone(),
                    client_id: request.client_id.clone(),
                    venue: venue.clone(),
                    instrument: request.instrument.as_str().to_string(),
                    side: request.side,
                    fills: 0,
                    quantity_scaled: 0,
                    notional_scaled: 0,
                    first_fill_ns: None,
                };
                keep(&mut self.orders, row);
            }
            ControlEvent::Filled { client_id, fill, .. } => {
                let (price, quantity) = (fill.price_scaled, fill.quantity_scaled);
                self.add_fill(client_id, price, quantity, fill.received_at_ns);
            }
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        if self.orders.is_empty() {
            return nothing(out, "No order has been recorded.");
        }
        table_head(out, &["decision", "side", "fills", "average price", "arrival cost bps"]);
        let table = self.rows().unwrap_or_else(|| Table::new(COLUMNS));
        for (r, cells) in self.orders.iter().zip(&table.rows) {
            let shown = |cell: &Cell| match cell {
                Cell::Text(v) => text(v),
                _ => "-".to_string(),
            };
            out.push_str(&format!(
                "<tr><td>{}<td>{:?}<td>{}<td>{}<td>{}</tr>",
                text(&r.decision),
                r.side,
                r.fills,
                shown(&cells[12]),
                shown(&cells[14])
            ));
        }
        out.push_str("</table>");
        out.push_str(
            "<p class=\"none\">Against the mid of the book when the strategy decided, before \
             fees; positive is a cost.</p>",
        );
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(COLUMNS);
        for r in &self.orders {
            let benchmark = self.benchmarks.get(&r.decision);
            let book = benchmark.and_then(|b| b.book.as_ref());
            let price = |ticks: Option<i64>| {
                self.scales.price(r.venue.as_deref(), &r.instrument, ticks.map(i128::from))
            };
            let decided = benchmark.map(|b| b.decided_at_ns);
            let reason = benchmark.filter(|b| b.book.is_none()).map(|b| b.detail.as_str());
            let mut row = vec![
                Cell::integer(decided),
                Cell::optional_text(r.venue.as_deref()),
                Cell::text(r.instrument.clone()),
                Cell::text(r.decision.clone()),
                Cell::text(r.client_id.clone()),
                Cell::text(format!("{:?}", r.side)),
                price(book.map(|b| b.best_bid_ticks)),
                price(book.map(|b| b.best_ask_ticks)),
                Cell::integer(book.map(|b| b.capture_lag_ns)),
                Cell::optional_text(reason.or(benchmark.is_none().then_some("NotCaptured"))),
                Cell::count(r.fills),
            ];
            let filled = (r.fills > 0).then_some(r.quantity_scaled);
            row.push(match filled {
                Some(q) => self.scales.quantity(r.venue.as_deref(), &r.instrument, q),
                None => Cell::Absent,
            });
            row.extend(self.cost_cells(r, benchmark));
            let latency = r.first_fill_ns.zip(decided).map(|(fill, decided)| fill - decided);
            row.push(Cell::integer(latency));
            table.rows.push(row);
        }
        Some(table)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/execution_tests.rs"]
mod tests;
