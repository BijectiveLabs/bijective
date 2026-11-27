// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the venues reported besides the book, from the newest files of the market archive: the
//! last trade, funding, open interest, mark and index per instrument, and the newest liquidations.
//!
//! The archive seals a file when a minute of received time closes, so these figures trail the
//! venue by about a minute, and a quiet feed seals later. The arrival column says how far behind
//! each row is, so a stale archive reads as stale.

use std::collections::BTreeMap;

use bijective_archive::{FactKind, FactRow, FactSide};
use bijective_common::clock::as_utc;

use crate::markup::{decimal, nothing, table_head, text};
use crate::panel::{Entry, Panel};
use crate::table::{Cell, Table, Unit};

/// Liquidations listed, newest first.
const LIQUIDATIONS: usize = 50;

/// One instrument on one venue, as its newest rows of each kind left it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reported {
    pub price_scale: u8,
    pub quantity_scale: u8,
    pub last_trade: Option<FactRow>,
    /// Trades this console has read since it started, not in any fixed span of time.
    pub trades: usize,
    pub funding: Option<FactRow>,
    pub open_interest: Option<FactRow>,
    pub open_interest_value: Option<FactRow>,
    pub mark: Option<FactRow>,
    pub index: Option<FactRow>,
    /// The newest arrival among this instrument's rows, on the platform clock.
    pub received_at_ns: i64,
}

#[derive(Default)]
pub struct ReportedPanel {
    /// By venue and instrument.
    pub reported: BTreeMap<(String, String), Reported>,
}

fn side_name(side: Option<FactSide>) -> &'static str {
    match side {
        Some(FactSide::Buyer) => "buyer",
        Some(FactSide::Seller) => "seller",
        Some(FactSide::Buy) => "buy",
        Some(FactSide::Sell) => "sell",
        None => "not stated",
    }
}

/// The figure a row carries in its own scale, or a dash when the column is absent.
fn figure(value: Option<i64>, scale: Option<u8>) -> String {
    match (value, scale) {
        (Some(v), Some(s)) => decimal(v, s),
        _ => "-".to_string(),
    }
}

fn scaled(value: Option<i64>, scale: u8) -> String {
    figure(value, Some(scale))
}

impl Panel for ReportedPanel {
    fn title(&self) -> &'static str {
        "Trades, funding and open interest"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Fact { venue, price_scale, quantity_scale, row } = entry else { return };
        if row.kind == FactKind::Liquidation {
            return;
        }
        let key = ((*venue).to_string(), row.instrument.to_string());
        let held = self.reported.entry(key).or_default();
        held.price_scale = *price_scale;
        held.quantity_scale = *quantity_scale;
        held.received_at_ns = held.received_at_ns.max(row.received_at_ns);
        let slot = match row.kind {
            FactKind::Trade => {
                held.trades += 1;
                &mut held.last_trade
            }
            FactKind::Funding => &mut held.funding,
            FactKind::OpenInterest => &mut held.open_interest,
            FactKind::OpenInterestValue => &mut held.open_interest_value,
            FactKind::Mark => &mut held.mark,
            FactKind::Index => &mut held.index,
            FactKind::Liquidation | FactKind::Resolution => return,
        };
        *slot = Some((*row).clone());
    }

    fn render(&self, out: &mut String) {
        if self.reported.is_empty() {
            return nothing(out, "No archive was named, or its newest files hold no such row.");
        }
        table_head(
            out,
            &[
                "venue",
                "instrument",
                "last trade",
                "size",
                "aggressor",
                "trades seen",
                "mark",
                "index",
                "funding",
                "next funding",
                "open interest",
                "newest arrival",
            ],
        );
        for ((venue, instrument), r) in &self.reported {
            out.push_str(&format!("<tr><td>{}<td>{}", text(venue), text(instrument)));
            row_cells(out, r);
            out.push_str("</tr>");
        }
        out.push_str("</table>");
        out.push_str(
            "<p class=\"none\">From the newest sealed files of the market archive. Funding is \
             the rate exactly as the venue wrote it; open interest is base quantity, then quote \
             value where the venue states one.</p>",
        );
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(REPORTED_COLUMNS);
        for ((venue, instrument), r) in &self.reported {
            let trade = r.last_trade.as_ref();
            let price = |row: Option<&FactRow>| at(row.and_then(|f| f.price_scaled), r.price_scale);
            let own = |row: &Option<FactRow>| {
                let row = row.as_ref();
                Cell::decimal(
                    row.and_then(|f| f.value_scaled).map(i128::from),
                    row.and_then(|f| f.value_scale),
                )
            };
            table.rows.push(vec![
                Cell::text(venue.clone()),
                Cell::text(instrument.clone()),
                price(trade),
                at(trade.and_then(|t| t.quantity_scaled), r.quantity_scale),
                Cell::optional_text(trade.map(|t| side_name(t.side))),
                Cell::count(r.trades),
                price(r.mark.as_ref()),
                price(r.index.as_ref()),
                own(&r.funding),
                Cell::integer(r.funding.as_ref().and_then(|f| f.next_funding_ns).map(i128::from)),
                at(r.open_interest.as_ref().and_then(|f| f.quantity_scaled), r.quantity_scale),
                own(&r.open_interest_value),
                Cell::Integer(i128::from(r.received_at_ns)),
            ]);
        }
        Some(table)
    }
}

/// Prices and sizes at the scales the archive file declares. Funding and open interest value at
/// their own.
const REPORTED_COLUMNS: &[(&str, Unit)] = &[
    ("venue", Unit::Text),
    ("instrument", Unit::Text),
    ("last trade", Unit::Decimal),
    ("size", Unit::Decimal),
    ("aggressor", Unit::Text),
    ("trades seen", Unit::Count),
    ("mark", Unit::Decimal),
    ("index", Unit::Decimal),
    ("funding", Unit::Decimal),
    ("next funding", Unit::Nanoseconds),
    ("open interest", Unit::Decimal),
    ("open interest value", Unit::Decimal),
    ("newest arrival", Unit::Nanoseconds),
];

const LIQUIDATION_COLUMNS: &[(&str, Unit)] = &[
    ("arrival", Unit::Nanoseconds),
    ("venue", Unit::Text),
    ("instrument", Unit::Text),
    ("forced order", Unit::Text),
    ("price", Unit::Decimal),
    ("size", Unit::Decimal),
];

/// A figure at a scale the file declared.
fn at(value: Option<i64>, scale: u8) -> Cell {
    Cell::decimal(value.map(i128::from), Some(scale))
}

fn row_cells(out: &mut String, r: &Reported) {
    let trade = r.last_trade.as_ref();
    let next_funding = r
        .funding
        .as_ref()
        .and_then(|f| f.next_funding_ns)
        .map_or_else(|| "-".to_string(), |at| as_utc(i128::from(at)));
    let open_interest = format!(
        "{} / {}",
        scaled(r.open_interest.as_ref().and_then(|f| f.quantity_scaled), r.quantity_scale),
        r.open_interest_value
            .as_ref()
            .map_or_else(|| "-".to_string(), |f| figure(f.value_scaled, f.value_scale)),
    );
    out.push_str(&format!(
        "<td>{}<td>{}<td>{}<td>{}<td>{}<td>{}<td>{}<td>{}<td>{}<td>{}",
        scaled(trade.and_then(|t| t.price_scaled), r.price_scale),
        scaled(trade.and_then(|t| t.quantity_scaled), r.quantity_scale),
        trade.map_or("-", |t| side_name(t.side)),
        r.trades,
        scaled(r.mark.as_ref().and_then(|f| f.price_scaled), r.price_scale),
        scaled(r.index.as_ref().and_then(|f| f.price_scaled), r.price_scale),
        r.funding
            .as_ref()
            .map_or_else(|| "-".to_string(), |f| figure(f.value_scaled, f.value_scale)),
        next_funding,
        open_interest,
        as_utc(i128::from(r.received_at_ns)),
    ));
}

/// A liquidation with the venue it came from and the scales to read it at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forced {
    pub venue: String,
    pub price_scale: u8,
    pub quantity_scale: u8,
    pub row: FactRow,
}

#[derive(Default)]
pub struct LiquidationsPanel {
    /// Oldest first, the newest `LIQUIDATIONS` only.
    pub forced: Vec<Forced>,
}

impl Panel for LiquidationsPanel {
    fn title(&self) -> &'static str {
        "Liquidations"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Fact { venue, price_scale, quantity_scale, row } = entry else { return };
        if row.kind != FactKind::Liquidation {
            return;
        }
        self.forced.push(Forced {
            venue: (*venue).to_string(),
            price_scale: *price_scale,
            quantity_scale: *quantity_scale,
            row: (*row).clone(),
        });
        if self.forced.len() > LIQUIDATIONS {
            self.forced.remove(0);
        }
    }

    fn render(&self, out: &mut String) {
        if self.forced.is_empty() {
            return nothing(out, "No liquidation in the newest files of the market archive.");
        }
        table_head(out, &["arrival", "venue", "instrument", "forced order", "price", "size"]);
        for f in self.forced.iter().rev() {
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{}<td>{}</tr>",
                as_utc(i128::from(f.row.received_at_ns)),
                text(&f.venue),
                text(&f.row.instrument),
                side_name(f.row.side),
                scaled(f.row.price_scaled, f.price_scale),
                scaled(f.row.quantity_scaled, f.quantity_scale),
            ));
        }
        out.push_str("</table>");
    }

    /// Newest first, as the page lists them.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(LIQUIDATION_COLUMNS);
        for f in self.forced.iter().rev() {
            table.rows.push(vec![
                Cell::Integer(i128::from(f.row.received_at_ns)),
                Cell::text(f.venue.clone()),
                Cell::text(f.row.instrument.to_string()),
                Cell::text(side_name(f.row.side)),
                at(f.row.price_scaled, f.price_scale),
                at(f.row.quantity_scaled, f.quantity_scale),
            ]);
        }
        Some(table)
    }
}
