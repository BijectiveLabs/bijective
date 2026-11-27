// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Proposals the platform holds for a parent's fill or a group (decision 0013), and what each has
//! had released as orders.
//!
//! Folded with the node's own projections, so the console reads a holding exactly as the node
//! that releases it does.

use bijective_control_engine::linked::{HoldState, Linked};
use bijective_control_engine::orders::Orders;
use bijective_event_store::projection::Projection;

use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

#[derive(Default)]
pub struct HeldPanel {
    linked: Linked,
    orders: Orders,
    /// An entry a projection refused: the holdings shown may then differ from the node's.
    pub refused: usize,
}

fn state_name(state: HoldState) -> &'static str {
    match state {
        HoldState::Waiting => "<strong>held</strong>",
        HoldState::Released => "released",
        HoldState::Displaced => "displaced by its group",
        HoldState::Ended => "ended by an operator",
    }
}

/// The state as one word a program can match on.
fn state_word(state: HoldState) -> &'static str {
    match state {
        HoldState::Waiting => "held",
        HoldState::Released => "released",
        HoldState::Displaced => "displaced",
        HoldState::Ended => "ended",
    }
}

const HELD_COLUMNS: &[(&str, Unit)] = &[
    ("proposal", Unit::Text),
    ("parent", Unit::Text),
    ("group", Unit::Text),
    ("instrument", Unit::Text),
    ("side", Unit::Text),
    ("quantity", Unit::Instrument),
    ("released", Unit::Instrument),
    ("state", Unit::Text),
];

impl Panel for HeldPanel {
    fn title(&self) -> &'static str {
        "Held"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(event) = entry else { return };
        let linked = self.linked.apply(event).is_ok();
        let orders = self.orders.apply(event).is_ok();
        if !(linked && orders) {
            self.refused += 1;
        }
    }

    fn render(&self, out: &mut String) {
        let holdings = self.linked.holdings(&self.orders);
        if holdings.is_empty() {
            return nothing(out, "No proposal has been held.");
        }
        table_head(
            out,
            &["proposal", "parent", "group", "instrument", "side", "quantity", "released", "state"],
        );
        let or_dash = |v: Option<&str>| v.map_or_else(|| "-".to_string(), text);
        for h in &holdings {
            let p = &h.proposal;
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{:?}<td>{}<td>{}<td>{}</tr>",
                text(&p.client_id),
                or_dash(p.parent.as_deref()),
                or_dash(p.oco.as_deref()),
                text(p.instrument.as_str()),
                p.side,
                p.quantity_scaled,
                h.released_scaled,
                state_name(h.state),
            ));
        }
        out.push_str("</table>");
        out.push_str(
            "<p class=\"none\">A held leg is released as its parent fills, through the same \
             checks as any proposal. While the node is halted it stays held.</p>",
        );
    }

    fn signals(&self) -> Vec<Signal> {
        let mut signals = Vec::new();
        let waiting = self
            .linked
            .holdings(&self.orders)
            .iter()
            .filter(|h| h.state == HoldState::Waiting)
            .count();
        if waiting > 0 {
            let value = waiting.to_string();
            signals.push(Signal { label: "held proposals", value, level: Level::Normal });
        }
        if self.refused > 0 {
            let value = self.refused.to_string();
            signals.push(Signal { label: "held view incomplete", value, level: Level::Alarm });
        }
        signals
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(HELD_COLUMNS);
        for h in &self.linked.holdings(&self.orders) {
            let p = &h.proposal;
            table.rows.push(vec![
                Cell::text(p.client_id.clone()),
                Cell::optional_text(p.parent.as_deref()),
                Cell::optional_text(p.oco.as_deref()),
                Cell::text(p.instrument.as_str()),
                Cell::text(format!("{:?}", p.side)),
                Cell::Integer(p.quantity_scaled),
                Cell::Integer(h.released_scaled),
                Cell::text(state_word(h.state)),
            ]);
        }
        Some(table)
    }
}
