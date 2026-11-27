// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Each strategy's latest performance per instrument, and the entries as they were written,
//! from the `performance` stream.
//!
//! The console computes none of it. It shows what a program wrote in the open format
//! (`crate::performance`), so an operator's own analytics are shown the same way as a vendor's.

use std::collections::BTreeMap;

use super::keep;

use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Panel};
use crate::performance::{MARKOUT_HORIZONS_S, Performance};
use crate::table::{Cell, Table, Unit};

const COLUMNS: &[(&str, Unit)] = &[
    ("at", Unit::Nanoseconds),
    ("run", Unit::Text),
    ("desk", Unit::Text),
    ("strategy", Unit::Text),
    ("venue", Unit::Text),
    ("instrument", Unit::Text),
    ("quote currency", Unit::Text),
    ("fills", Unit::Count),
    ("wins", Unit::Count),
    ("losses", Unit::Count),
    ("position", Unit::Decimal),
    ("traded notional", Unit::Decimal),
    ("realized", Unit::Decimal),
    ("unrealized", Unit::Decimal),
    ("fees", Unit::Decimal),
    ("fees excluded", Unit::Text),
    ("net", Unit::Decimal),
    ("peak net", Unit::Decimal),
    ("drawdown", Unit::Decimal),
    ("arrival cost", Unit::Decimal),
    ("arrival cost bps", Unit::Decimal),
    ("markout 1s bps", Unit::Decimal),
    ("markout 5s bps", Unit::Decimal),
    ("markout 10s bps", Unit::Decimal),
    ("markout 30s bps", Unit::Decimal),
    ("markout 60s bps", Unit::Decimal),
    ("markout 60s fills", Unit::Count),
    ("source", Unit::Text),
    ("through", Unit::Count),
];

/// Run, desk, strategy, venue and instrument.
type Subject = (String, String, Option<String>, String, String);

/// The newest entry per run, desk, strategy, venue and instrument.
#[derive(Default)]
pub struct StrategiesPanel {
    pub latest: BTreeMap<Subject, (String, Performance)>,
}

fn optional(value: Option<&String>) -> Cell {
    Cell::optional_text(value.map(String::as_str))
}

fn markout_cells(p: &Performance) -> Vec<Cell> {
    let at = |h: u64| p.markouts.iter().find(|m| m.horizon_s == h);
    let mut cells: Vec<Cell> =
        MARKOUT_HORIZONS_S.iter().map(|h| optional(at(*h).and_then(|m| m.bps.as_ref()))).collect();
    let longest = MARKOUT_HORIZONS_S[MARKOUT_HORIZONS_S.len() - 1];
    cells.push(at(longest).map_or(Cell::Absent, |m| Cell::Integer(i128::from(m.fills))));
    cells
}

fn cells(run: &str, p: &Performance) -> Vec<Cell> {
    let mut row = vec![
        Cell::Integer(p.at_ns),
        Cell::text(run),
        Cell::text(p.desk.clone()),
        optional(p.strategy.as_ref()),
        Cell::text(p.venue.clone()),
        Cell::text(p.identity.clone().unwrap_or_else(|| p.instrument.clone())),
        optional(p.quote_currency.as_ref()),
        Cell::Integer(i128::from(p.fills)),
        Cell::Integer(i128::from(p.wins)),
        Cell::Integer(i128::from(p.losses)),
        Cell::text(p.position.clone()),
        Cell::text(p.traded_notional.clone()),
        Cell::text(p.realized.clone()),
        optional(p.unrealized.as_ref()),
        Cell::text(p.fees.clone()),
        Cell::text(p.fees_excluded.join(" ")),
        optional(p.net.as_ref()),
        optional(p.peak_net.as_ref()),
        optional(p.drawdown.as_ref()),
        optional(p.arrival_cost.as_ref()),
        optional(p.arrival_cost_bps.as_ref()),
    ];
    row.extend(markout_cells(p));
    row.push(Cell::text(p.source.clone()));
    row.push(Cell::Integer(i128::from(p.through)));
    row
}

impl Panel for StrategiesPanel {
    fn title(&self) -> &'static str {
        "Strategy performance"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Performance { run, performance: p } = entry else { return };
        let key = (
            (*run).to_string(),
            p.desk.clone(),
            p.strategy.clone(),
            p.venue.clone(),
            p.instrument.clone(),
        );
        self.latest.insert(key, ((*run).to_string(), (*p).clone()));
    }

    fn render(&self, out: &mut String) {
        if self.latest.is_empty() {
            return nothing(out, "No program has written strategy performance for this node.");
        }
        table_head(
            out,
            &["run", "strategy", "instrument", "fills", "net", "drawdown", "markout 10s bps"],
        );
        let dash = |v: Option<&String>| v.map_or_else(|| "-".to_string(), |v| text(v));
        for (run, p) in self.latest.values() {
            let ten = p.markouts.iter().find(|m| m.horizon_s == 10).and_then(|m| m.bps.as_ref());
            out.push_str(&format!(
                "<tr><td>{}<td>{}/{}<td>{}<td>{}<td>{}<td>{}<td>{}</tr>",
                text(run),
                text(&p.desk),
                dash(p.strategy.as_ref()),
                text(&p.instrument),
                p.fills,
                dash(p.net.as_ref()),
                dash(p.drawdown.as_ref()),
                dash(ten),
            ));
        }
        out.push_str("</table>");
        out.push_str(
            "<p class=\"none\">Net of fees in the quote currency; a markout is positive in the \
             fill's favour.</p>",
        );
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(COLUMNS);
        table.rows.extend(self.latest.values().map(|(run, p)| cells(run, p)));
        Some(table)
    }
}

/// The newest entries as written, oldest first: the course of each strategy's figures.
#[derive(Default)]
pub struct StrategyHistoryPanel {
    pub written: Vec<(String, Performance)>,
}

impl Panel for StrategyHistoryPanel {
    fn title(&self) -> &'static str {
        "Strategy performance history"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        if let Entry::Performance { run, performance } = entry {
            keep(&mut self.written, ((*run).to_string(), (*performance).clone()));
        }
    }

    fn render(&self, out: &mut String) {
        out.push_str(&format!("<p class=\"none\">{} entries kept.</p>", self.written.len()));
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(COLUMNS);
        table.rows.extend(self.written.iter().map(|(run, p)| cells(run, p)));
        Some(table)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/strategies_tests.rs"]
mod tests;
