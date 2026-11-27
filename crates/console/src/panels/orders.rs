// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Orders as the control stream records them, the decisions refused, and the benchmarks captured.

use std::collections::BTreeMap;

use bijective_control_engine::wire::{ControlEvent, RefusalReason};
use bijective_model::{OrderRequest, Side};
use bijective_venue::normalise::{Attempted, VenueOutcome};

use super::keep;
use super::scales::Scales;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settlement {
    pub outcome: VenueOutcome,
    pub at_ns: i128,
    /// Whether the venue's answer left the order's fate open.
    pub fate_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRow {
    pub client_id: String,
    /// Absent on an intent recorded before intents named their venue.
    pub venue: Option<String>,
    pub instrument: String,
    pub side: Side,
    pub quantity_scaled: i128,
    pub price_scaled: Option<i128>,
    pub intent_at_ns: i128,
    /// Absent while the venue has not answered. Shown as unanswered, never omitted.
    pub settled: Option<Settlement>,
    pub withdrawn: Option<Settlement>,
    /// The sum of fills recorded for it, at the quantity scale.
    pub filled_scaled: i128,
}

#[derive(Default)]
pub struct OrdersPanel {
    pub orders: Vec<OrderRow>,
    scales: Scales,
    pub orders_seen: usize,
    by_client: BTreeMap<String, usize>,
}

impl OrdersPanel {
    fn intent(&mut self, venue: Option<&str>, request: &OrderRequest, at_ns: i128) {
        self.by_client.insert(request.client_id.clone(), self.orders_seen);
        self.orders_seen += 1;
        let row = OrderRow {
            client_id: request.client_id.clone(),
            venue: venue.map(str::to_string),
            instrument: request.instrument.as_str().to_string(),
            side: request.side,
            quantity_scaled: request.quantity_scaled,
            price_scaled: request.price_scaled,
            intent_at_ns: at_ns,
            settled: None,
            withdrawn: None,
            filled_scaled: 0,
        };
        keep(&mut self.orders, row);
    }

    /// The row an identifier names, if still in the kept tail. Completing the wrong row is worse
    /// than showing nothing.
    fn row_for(&mut self, client_id: &str) -> Option<&mut OrderRow> {
        let at = *self.by_client.get(client_id)?;
        let dropped = self.orders_seen.saturating_sub(self.orders.len());
        self.orders.get_mut(at.checked_sub(dropped)?)
    }
}

fn answered(settlement: &Option<Settlement>) -> String {
    match settlement {
        None => "<em>unanswered</em>".to_string(),
        Some(s) if s.fate_open => {
            format!("{} <strong>fate_open</strong>", text(&format!("{:?}", s.outcome)))
        }
        Some(s) => text(&format!("{:?}", s.outcome)),
    }
}

/// The venue's answer, when it came, and whether it left the order's fate open. All absent while
/// unanswered.
fn answer_cells(settlement: &Option<Settlement>, row: &mut Vec<Cell>) {
    match settlement {
        None => row.extend([Cell::Absent, Cell::Absent, Cell::Absent]),
        Some(s) => row.extend([
            Cell::text(format!("{:?}", s.outcome)),
            Cell::Integer(s.at_ns),
            Cell::Flag(s.fate_open),
        ]),
    }
}

const ORDER_COLUMNS: &[(&str, Unit)] = &[
    ("client id", Unit::Text),
    ("instrument", Unit::Text),
    ("side", Unit::Text),
    ("venue", Unit::Text),
    ("quantity", Unit::Decimal),
    ("filled", Unit::Decimal),
    ("limit price", Unit::Decimal),
    ("intent at", Unit::Nanoseconds),
    ("settled", Unit::Text),
    ("settled at", Unit::Nanoseconds),
    ("settled fate open", Unit::Flag),
    ("withdrawn", Unit::Text),
    ("withdrawn at", Unit::Nanoseconds),
    ("withdrawn fate open", Unit::Flag),
];

impl Panel for OrdersPanel {
    fn title(&self) -> &'static str {
        "Orders"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(event) = entry else { return };
        self.scales.learn(event);
        match event {
            ControlEvent::Intent { venue, request, at_ns, .. } => {
                self.intent(venue.as_deref(), request, *at_ns);
            }
            ControlEvent::Settled { client_id, outcome, at_ns } => {
                let fate_open = outcome.needs_reconciliation(Attempted::Submit);
                if let Some(row) = self.row_for(client_id) {
                    let outcome = outcome.clone();
                    row.settled = Some(Settlement { outcome, at_ns: *at_ns, fate_open });
                }
            }
            ControlEvent::Withdrawn { client_id, outcome, at_ns, .. } => {
                let fate_open = outcome.needs_reconciliation(Attempted::Cancel);
                if let Some(row) = self.row_for(client_id) {
                    let outcome = outcome.clone();
                    row.withdrawn = Some(Settlement { outcome, at_ns: *at_ns, fate_open });
                }
            }
            ControlEvent::Filled { client_id, fill, .. } => {
                if let Some(row) = self.row_for(client_id) {
                    row.filled_scaled += fill.quantity_scaled;
                }
            }
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        out.push_str(&format!(
            "<p class=\"none\">{} placed, {} shown</p>",
            self.orders_seen,
            self.orders.len()
        ));
        if self.orders.is_empty() {
            return nothing(out, "No order has been placed.");
        }
        table_head(
            out,
            &[
                "client id",
                "instrument",
                "side",
                "quantity",
                "filled",
                "price",
                "intent at",
                "settled",
                "withdrawn",
            ],
        );
        for o in &self.orders {
            let price =
                o.price_scaled.map_or_else(|| "<em>market</em>".to_string(), |p| p.to_string());
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{:?}<td>{}<td>{}<td>{price}<td>{}<td>{}<td>{}</tr>",
                text(&o.client_id),
                text(&o.instrument),
                o.side,
                o.quantity_scaled,
                o.filled_scaled,
                o.intent_at_ns,
                answered(&o.settled),
                answered(&o.withdrawn),
            ));
        }
        out.push_str("</table>");
    }

    fn signals(&self) -> Vec<Signal> {
        let open = |s: &Option<Settlement>| s.as_ref().is_some_and(|s| s.fate_open);
        let count = self.orders.iter().filter(|o| open(&o.settled) || open(&o.withdrawn)).count();
        if count == 0 {
            return Vec::new();
        }
        vec![Signal {
            label: "orders with an open fate",
            value: count.to_string(),
            level: Level::Alarm,
        }]
    }

    /// A market order's limit price is absent.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(ORDER_COLUMNS);
        for o in &self.orders {
            let mut row = vec![
                Cell::text(o.client_id.clone()),
                Cell::text(o.instrument.clone()),
                Cell::text(format!("{:?}", o.side)),
                Cell::optional_text(o.venue.as_deref()),
                self.scales.quantity(o.venue.as_deref(), &o.instrument, o.quantity_scaled),
                self.scales.quantity(o.venue.as_deref(), &o.instrument, o.filled_scaled),
                self.scales.price(o.venue.as_deref(), &o.instrument, o.price_scaled),
                Cell::Integer(o.intent_at_ns),
            ];
            answer_cells(&o.settled, &mut row);
            answer_cells(&o.withdrawn, &mut row);
            table.rows.push(row);
        }
        Some(table)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedRow {
    pub decision_id: String,
    pub reason: RefusalReason,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct RefusedPanel {
    pub refused: Vec<RefusedRow>,
    pub refused_seen: usize,
}

impl Panel for RefusedPanel {
    fn title(&self) -> &'static str {
        "Refused"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::Refused { decision_id, reason, at_ns, .. }) = entry else {
            return;
        };
        self.refused_seen += 1;
        let row =
            RefusedRow { decision_id: decision_id.clone(), reason: reason.clone(), at_ns: *at_ns };
        keep(&mut self.refused, row);
    }

    fn render(&self, out: &mut String) {
        out.push_str(&format!(
            "<p class=\"none\">{} refused, {} shown</p>",
            self.refused_seen,
            self.refused.len()
        ));
        if self.refused.is_empty() {
            return;
        }
        table_head(out, &["decision", "reason", "at"]);
        for r in &self.refused {
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}</tr>",
                text(&r.decision_id),
                text(&format!("{:?}", r.reason)),
                r.at_ns
            ));
        }
        out.push_str("</table>");
    }

    /// The kept tail. The count since the stream began is on the page.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("decision", Unit::Text),
            ("reason", Unit::Text),
            ("at", Unit::Nanoseconds),
        ]);
        for r in &self.refused {
            table.rows.push(vec![
                Cell::text(r.decision_id.clone()),
                Cell::text(format!("{:?}", r.reason)),
                Cell::Integer(r.at_ns),
            ]);
        }
        Some(table)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    pub correlation_id: String,
    /// Absent where no benchmark could be taken, which the record distinguishes from zero.
    pub book_taken: bool,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct DecisionsPanel {
    pub decisions: Vec<DecisionRow>,
}

impl Panel for DecisionsPanel {
    fn title(&self) -> &'static str {
        "Decisions"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::Captured { correlation_id, book, at_ns, .. }) = entry
        else {
            return;
        };
        let row = DecisionRow {
            correlation_id: correlation_id.clone(),
            book_taken: book.is_some(),
            at_ns: *at_ns,
        };
        keep(&mut self.decisions, row);
    }

    fn render(&self, out: &mut String) {
        if self.decisions.is_empty() {
            return nothing(out, "No decision has been captured.");
        }
        table_head(out, &["correlation", "benchmark", "at"]);
        for d in &self.decisions {
            let book = if d.book_taken { "taken" } else { "<em>none could be taken</em>" };
            out.push_str(&format!(
                "<tr><td>{}<td>{book}<td>{}</tr>",
                text(&d.correlation_id),
                d.at_ns
            ));
        }
        out.push_str("</table>");
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("correlation", Unit::Text),
            ("benchmark taken", Unit::Flag),
            ("at", Unit::Nanoseconds),
        ]);
        for d in &self.decisions {
            table.rows.push(vec![
                Cell::text(d.correlation_id.clone()),
                Cell::Flag(d.book_taken),
                Cell::Integer(d.at_ns),
            ]);
        }
        Some(table)
    }
}
