// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The newest top of book per instrument and venue, from the market streams' tails.

use std::collections::BTreeMap;

use crate::markup::{nothing, sparkline, table_head, text};
use crate::panel::{Entry, Panel};
use crate::table::{Cell, Table, Unit};

/// Mid points kept per instrument for its line. The console reads no more than a tail anyway.
const MIDS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Top {
    pub bid_price: i128,
    pub ask_price: i128,
    pub bid_depth: u32,
    pub ask_depth: u32,
    pub venue_ts_ms: u64,
    pub mids: Vec<i128>,
}

#[derive(Default)]
pub struct MarketPanel {
    /// By venue and instrument identity.
    pub tops: BTreeMap<(String, String), Top>,
}

/// The spread in basis points of the mid, or nothing when a side is empty.
fn spread_bps(bid: i128, ask: i128) -> Option<i128> {
    let mid = (bid + ask) / 2;
    (bid > 0 && ask > 0 && mid > 0).then(|| (ask - bid) * 10_000 / mid)
}

impl Panel for MarketPanel {
    fn title(&self) -> &'static str {
        "Top of book"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Book { venue, update } = entry else { return };
        let key = ((*venue).to_string(), update.instrument.as_str().to_string());
        let top = self.tops.entry(key).or_insert(Top {
            bid_price: 0,
            ask_price: 0,
            bid_depth: 0,
            ask_depth: 0,
            venue_ts_ms: 0,
            mids: Vec::new(),
        });
        top.bid_price = update.bid_price;
        top.ask_price = update.ask_price;
        top.bid_depth = update.bid_depth;
        top.ask_depth = update.ask_depth;
        top.venue_ts_ms = update.venue_ts_ms;
        if update.bid_price > 0 && update.ask_price > 0 {
            top.mids.push((update.bid_price + update.ask_price) / 2);
            if top.mids.len() > MIDS {
                top.mids.remove(0);
            }
        }
    }

    fn render(&self, out: &mut String) {
        if self.tops.is_empty() {
            return nothing(out, "No market stream holds a top of book.");
        }
        table_head(
            out,
            &["venue", "instrument", "bid", "ask", "spread bps", "depth", "venue time", "mid"],
        );
        for ((venue, instrument), t) in &self.tops {
            let spread = spread_bps(t.bid_price, t.ask_price)
                .map_or_else(|| "<em>one side empty</em>".to_string(), |s| s.to_string());
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{spread}<td>{} / {}<td>{}<td>{}</tr>",
                text(venue),
                text(instrument),
                t.bid_price,
                t.ask_price,
                t.bid_depth,
                t.ask_depth,
                t.venue_ts_ms,
                sparkline(&t.mids)
            ));
        }
        out.push_str("</table>");
        out.push_str(
            "<p class=\"none\">Prices at the venue's own scale. The newest entries of each \
             market stream only.</p>",
        );
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(TOP_COLUMNS);
        for ((venue, instrument), t) in &self.tops {
            let venue_ns = i128::from(t.venue_ts_ms) * 1_000_000;
            table.rows.push(vec![
                Cell::text(venue.clone()),
                Cell::text(instrument.clone()),
                Cell::Integer(t.bid_price),
                Cell::Integer(t.ask_price),
                Cell::integer(spread_bps(t.bid_price, t.ask_price)),
                Cell::Integer(i128::from(t.bid_depth)),
                Cell::Integer(i128::from(t.ask_depth)),
                Cell::Integer(venue_ns),
            ]);
        }
        Some(table)
    }
}

/// Prices at the venue's own scale. The spread is absent while a side is empty.
const TOP_COLUMNS: &[(&str, Unit)] = &[
    ("venue", Unit::Text),
    ("instrument", Unit::Text),
    ("bid", Unit::Instrument),
    ("ask", Unit::Instrument),
    ("spread bps", Unit::Count),
    ("bid depth", Unit::Count),
    ("ask depth", Unit::Count),
    ("venue time", Unit::Nanoseconds),
];
