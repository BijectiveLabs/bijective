// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Fills of the platform's orders as the venues reported them, and the account charges that name
//! no order (funding, and fees a venue books apart from its fills).
//!
//! Both are what the control stream recorded once per venue identity, so a fill or a charge read
//! twice is listed once.

use std::collections::BTreeMap;

use bijective_control_engine::wire::ControlEvent;
use bijective_lifecycle::{Fee, Fill};
use bijective_model::market::ExchangeTime;

use super::keep;
use super::scales::Scales;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Panel};
use crate::table::{Cell, Table, Unit};

/// A fill with the venue and instrument of the order it filled, where the intent was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillRow {
    pub client_id: String,
    /// Absent where the intent is not in the stream or did not name its venue.
    pub venue: Option<String>,
    pub instrument: Option<String>,
    pub fill: Fill,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct FillsPanel {
    pub fills: Vec<FillRow>,
    pub fills_seen: usize,
    /// The venue and instrument of every intent, by client identifier.
    orders: BTreeMap<String, (Option<String>, String)>,
    scales: Scales,
}

const FILL_COLUMNS: &[(&str, Unit)] = &[
    ("recorded at", Unit::Nanoseconds),
    ("exchange time", Unit::Nanoseconds),
    ("venue", Unit::Text),
    ("client id", Unit::Text),
    ("instrument", Unit::Text),
    ("side", Unit::Text),
    ("quantity", Unit::Decimal),
    ("price", Unit::Decimal),
    ("fee", Unit::Decimal),
    ("fee asset", Unit::Text),
    ("liquidity", Unit::Text),
    ("fill id", Unit::Text),
    ("venue order id", Unit::Text),
];

fn exchange_ns(time: &ExchangeTime) -> Option<i128> {
    match time {
        ExchangeTime::At { at_ns } => Some(*at_ns),
        ExchangeTime::NotPublished => None,
    }
}

/// A fee as the exact decimal the venue charged, positive paid and negative a rebate.
fn fee_cells(fee: Option<&Fee>) -> [Cell; 2] {
    match fee {
        Some(f) => {
            [Cell::decimal(Some(f.amount_scaled), Some(f.scale)), Cell::text(f.asset.clone())]
        }
        None => [Cell::Absent, Cell::Absent],
    }
}

impl FillsPanel {
    /// Absent where the order's intent, and so its instrument, was not read.
    fn quantity(&self, r: &FillRow, scaled: i128) -> Cell {
        let Some(instrument) = r.instrument.as_deref() else { return Cell::Absent };
        self.scales.quantity(r.venue.as_deref(), instrument, scaled)
    }

    fn price(&self, r: &FillRow, scaled: i128) -> Cell {
        let Some(instrument) = r.instrument.as_deref() else { return Cell::Absent };
        self.scales.price(r.venue.as_deref(), instrument, Some(scaled))
    }
}

impl Panel for FillsPanel {
    fn title(&self) -> &'static str {
        "Fills"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(event) = entry else { return };
        self.scales.learn(event);
        match event {
            ControlEvent::Intent { venue, request, .. } => {
                let instrument = request.instrument.as_str().to_string();
                self.orders.insert(request.client_id.clone(), (venue.clone(), instrument));
            }
            ControlEvent::Filled { client_id, fill, at_ns } => {
                self.fills_seen += 1;
                let order = self.orders.get(client_id);
                let row = FillRow {
                    client_id: client_id.clone(),
                    venue: order.and_then(|(venue, _)| venue.clone()),
                    instrument: order.map(|(_, instrument)| instrument.clone()),
                    fill: fill.clone(),
                    at_ns: *at_ns,
                };
                keep(&mut self.fills, row);
            }
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        out.push_str(&format!(
            "<p class=\"none\">{} fills recorded, {} shown</p>",
            self.fills_seen,
            self.fills.len()
        ));
        if self.fills.is_empty() {
            return;
        }
        table_head(
            out,
            &["client id", "venue", "side", "quantity", "price", "fee", "liquidity", "at"],
        );
        let or_dash = |v: Option<&str>| v.map_or_else(|| "-".to_string(), text);
        for r in &self.fills {
            let fee = r.fill.fee.as_ref().map_or_else(
                || "-".to_string(),
                |f| {
                    format!(
                        "{} {}",
                        crate::markup::decimal(f.amount_scaled, f.scale),
                        text(&f.asset)
                    )
                },
            );
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{:?}<td>{}<td>{}<td>{fee}<td>{:?}<td>{}</tr>",
                text(&r.client_id),
                or_dash(r.venue.as_deref()),
                r.fill.side,
                r.fill.quantity_scaled,
                r.fill.price_scaled,
                r.fill.liquidity,
                r.at_ns
            ));
        }
        out.push_str("</table>");
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(FILL_COLUMNS);
        for r in &self.fills {
            let f = &r.fill;
            let [fee, fee_asset] = fee_cells(f.fee.as_ref());
            table.rows.push(vec![
                Cell::Integer(r.at_ns),
                Cell::integer(exchange_ns(&f.exchange_time)),
                Cell::optional_text(r.venue.as_deref()),
                Cell::text(r.client_id.clone()),
                Cell::optional_text(r.instrument.as_deref()),
                Cell::text(format!("{:?}", f.side)),
                self.quantity(r, f.quantity_scaled),
                self.price(r, f.price_scaled),
                fee,
                fee_asset,
                Cell::text(format!("{:?}", f.liquidity)),
                Cell::text(f.fill_id.clone()),
                Cell::optional_text(f.venue_order_id.as_deref()),
            ]);
        }
        Some(table)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChargeRow {
    pub venue: String,
    pub charge_id: String,
    pub fee: Fee,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct ChargesPanel {
    pub charges: Vec<ChargeRow>,
}

impl Panel for ChargesPanel {
    fn title(&self) -> &'static str {
        "Account charges"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::AccountCharged { venue, charge, at_ns }) = entry else {
            return;
        };
        let row = ChargeRow {
            venue: venue.clone(),
            charge_id: charge.charge_id.clone(),
            fee: charge.fee.clone(),
            at_ns: *at_ns,
        };
        keep(&mut self.charges, row);
    }

    fn render(&self, out: &mut String) {
        if self.charges.is_empty() {
            return nothing(out, "No account charge has been recorded.");
        }
        table_head(out, &["venue", "charge", "amount", "asset", "at"]);
        for c in &self.charges {
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{}</tr>",
                text(&c.venue),
                text(&c.charge_id),
                crate::markup::decimal(c.fee.amount_scaled, c.fee.scale),
                text(&c.fee.asset),
                c.at_ns
            ));
        }
        out.push_str("</table>");
        out.push_str(
            "<p class=\"none\">Positive is paid, negative received. The daily loss counts a \
             charge on the day it is recorded.</p>",
        );
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("venue", Unit::Text),
            ("charge", Unit::Text),
            ("amount", Unit::Decimal),
            ("asset", Unit::Text),
            ("at", Unit::Nanoseconds),
        ]);
        for c in &self.charges {
            let [amount, asset] = fee_cells(Some(&c.fee));
            table.rows.push(vec![
                Cell::text(c.venue.clone()),
                Cell::text(c.charge_id.clone()),
                amount,
                asset,
                Cell::Integer(c.at_ns),
            ]);
        }
        Some(table)
    }
}
