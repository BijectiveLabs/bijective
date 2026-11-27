// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The facts table: every market record that is not a book, one row each.
//!
//! One table rather than one per kind: the kinds share their identity columns, a reader filters by
//! `kind`, and a kind added later adds rows, not files. A column a kind does not use is absent in
//! its rows, never zero. A kind no adapter produces yet (the long/short ratio) has no row and is
//! counted by the writer as not archived. Its frames are in the journal.

use std::sync::Arc;

use bijective_model::Side;
use bijective_model::market::{
    Aggressor, ExchangeTime, Funding, MarketFact, MarketRecord, OpenInterest, ScaledDecimal,
};

use crate::columns::{Column, ints, opt_i64, optional, required, rows_in, texts};
use crate::error::ArchiveError;
use crate::rows::narrow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Trade,
    Liquidation,
    Funding,
    /// In base quantity.
    OpenInterest,
    /// In quote value, where the venue states it so. Never converted from base quantity.
    OpenInterestValue,
    Mark,
    Index,
    /// `value_scaled` is 1 for the outcome that won and 0 for one that lost, at scale 0.
    Resolution,
}

/// The aggressor of a trade, or the side of a liquidation's forced order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactSide {
    Buyer,
    Seller,
    Buy,
    Sell,
}

const KINDS: [(&str, FactKind); 8] = [
    ("trade", FactKind::Trade),
    ("liquidation", FactKind::Liquidation),
    ("funding", FactKind::Funding),
    ("open_interest", FactKind::OpenInterest),
    ("open_interest_value", FactKind::OpenInterestValue),
    ("mark", FactKind::Mark),
    ("index", FactKind::Index),
    ("resolution", FactKind::Resolution),
];

const SIDES: [(&str, FactSide); 4] = [
    ("buyer", FactSide::Buyer),
    ("seller", FactSide::Seller),
    ("buy", FactSide::Buy),
    ("sell", FactSide::Sell),
];

fn name_of<T: PartialEq + Copy>(table: &[(&'static str, T)], value: T) -> &'static str {
    table.iter().find(|(_, v)| *v == value).map_or("", |(n, _)| n)
}

fn value_of<T: Copy>(table: &[(&str, T)], name: &str, column: &str) -> Result<T, String> {
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
        .ok_or_else(|| format!("{column} holds {name}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactRow {
    pub journal_seq: u64,
    pub record_in_frame: u32,
    pub received_at_ns: i64,
    pub exchange_time_ns: Option<i64>,
    pub instrument: Arc<str>,
    pub kind: FactKind,
    pub side: Option<FactSide>,
    pub trade_id: Option<String>,
    /// At the venue's price scale: a trade's or liquidation's price, or the mark or index price.
    pub price_scaled: Option<i64>,
    /// At the venue's quantity scale: a trade's or liquidation's size, or open interest in base.
    pub quantity_scaled: Option<i64>,
    /// A figure with its own scale: the funding rate as written, or open interest in quote.
    pub value_scaled: Option<i64>,
    pub value_scale: Option<u8>,
    pub next_funding_ns: Option<i64>,
}

fn at(time: ExchangeTime, field: &'static str) -> Result<Option<i64>, ArchiveError> {
    match time {
        ExchangeTime::At { at_ns } => Ok(Some(narrow(field, at_ns)?)),
        ExchangeTime::NotPublished => Ok(None),
    }
}

/// The row for a record that is not a book, `None` for a kind with no columns yet.
/// Precondition: the record is not a book snapshot or delta. Those are `book` rows.
pub fn fact_row(
    record: &MarketRecord,
    journal_seq: u64,
    record_in_frame: u32,
) -> Result<Option<FactRow>, ArchiveError> {
    let mut row = FactRow {
        journal_seq,
        record_in_frame,
        received_at_ns: narrow("received time", record.received_at_ns)?,
        exchange_time_ns: at(record.exchange_time, "exchange time")?,
        instrument: Arc::from(record.instrument.as_str()),
        kind: FactKind::Mark,
        side: None,
        trade_id: None,
        price_scaled: None,
        quantity_scaled: None,
        value_scaled: None,
        value_scale: None,
        next_funding_ns: None,
    };
    let decimal =
        |d: ScaledDecimal| narrow("value", d.value_scaled).map(|v| (Some(v), Some(d.scale)));
    match &record.fact {
        MarketFact::Trade(t) => {
            row.kind = FactKind::Trade;
            row.side = match t.aggressor {
                Aggressor::Buyer => Some(FactSide::Buyer),
                Aggressor::Seller => Some(FactSide::Seller),
                Aggressor::NotPublished => None,
            };
            row.trade_id = Some(t.trade_id.clone());
            row.price_scaled = Some(narrow("price", t.price_scaled)?);
            row.quantity_scaled = Some(narrow("quantity", t.quantity_scaled)?);
        }
        MarketFact::Liquidation(l) => {
            row.kind = FactKind::Liquidation;
            row.side = Some(match l.forced_order_side {
                Side::Buy => FactSide::Buy,
                Side::Sell => FactSide::Sell,
            });
            row.price_scaled = Some(narrow("price", l.price_scaled)?);
            row.quantity_scaled = Some(narrow("quantity", l.quantity_scaled)?);
        }
        MarketFact::Funding(Funding { rate, next_funding_time }) => {
            row.kind = FactKind::Funding;
            (row.value_scaled, row.value_scale) = decimal(*rate)?;
            row.next_funding_ns = at(*next_funding_time, "next funding time")?;
        }
        MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled }) => {
            row.kind = FactKind::OpenInterest;
            row.quantity_scaled = Some(narrow("quantity", *quantity_scaled)?);
        }
        MarketFact::OpenInterest(OpenInterest::QuoteValue { value }) => {
            row.kind = FactKind::OpenInterestValue;
            (row.value_scaled, row.value_scale) = decimal(*value)?;
        }
        MarketFact::MarkPrice { mark_price_scaled } => {
            row.kind = FactKind::Mark;
            row.price_scaled = Some(narrow("price", *mark_price_scaled)?);
        }
        MarketFact::IndexPrice { index_price_scaled } => {
            row.kind = FactKind::Index;
            row.price_scaled = Some(narrow("price", *index_price_scaled)?);
        }
        MarketFact::LongShortRatio(_) => return Ok(None),
        MarketFact::Resolution { won } => {
            row.kind = FactKind::Resolution;
            (row.value_scaled, row.value_scale) = (Some(i64::from(*won)), Some(0));
        }
        MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_) => {
            return Err(ArchiveError::NoTable { class: "book record in the facts table" });
        }
    }
    Ok(Some(row))
}

fn text_column(values: impl Iterator<Item = Option<String>>) -> Column {
    let (values, defs) = optional(values);
    Column::Text { values, defs: Some(defs) }
}

pub fn fact_columns(rows: &[FactRow]) -> Vec<Column> {
    vec![
        Column::I64 { values: rows.iter().map(|r| r.journal_seq as i64).collect(), defs: None },
        Column::I32 { values: rows.iter().map(|r| r.record_in_frame as i32).collect(), defs: None },
        Column::I64 { values: rows.iter().map(|r| r.received_at_ns).collect(), defs: None },
        opt_i64(rows.iter().map(|r| r.exchange_time_ns)),
        Column::Text {
            values: rows.iter().map(|r| r.instrument.to_string()).collect(),
            defs: None,
        },
        Column::Text {
            values: rows.iter().map(|r| name_of(&KINDS, r.kind).to_string()).collect(),
            defs: None,
        },
        text_column(rows.iter().map(|r| r.side.map(|s| name_of(&SIDES, s).to_string()))),
        text_column(rows.iter().map(|r| r.trade_id.clone())),
        opt_i64(rows.iter().map(|r| r.price_scaled)),
        opt_i64(rows.iter().map(|r| r.quantity_scaled)),
        opt_i64(rows.iter().map(|r| r.value_scaled)),
        {
            let (values, defs) = optional(rows.iter().map(|r| r.value_scale.map(i32::from)));
            Column::I32 { values, defs: Some(defs) }
        },
        opt_i64(rows.iter().map(|r| r.next_funding_ns)),
    ]
}

/// The facts table has 13 columns, in `fact_columns` order.
pub fn fact_rows_from(columns: &[Column]) -> Result<Vec<FactRow>, String> {
    if columns.len() != 13 {
        return Err(format!("{} columns, the facts table has 13", columns.len()));
    }
    let n = rows_in(columns);
    let c: Vec<Vec<Option<i64>>> = [0, 1, 2, 3, 8, 9, 10, 11, 12]
        .iter()
        .map(|i| ints(&columns[*i], n, "integer column"))
        .collect::<Result<_, _>>()?;
    let t: Vec<Vec<Option<String>>> = [4, 5, 6, 7]
        .iter()
        .map(|i| texts(&columns[*i], n, "text column"))
        .collect::<Result<_, _>>()?;
    (0..n)
        .map(|i| {
            let kind = required(t[1][i].as_deref(), "kind")?;
            let side = t[2][i].as_deref().map(|s| value_of(&SIDES, s, "side")).transpose()?;
            let value_scale = c[7][i]
                .map(|s| u8::try_from(s).map_err(|_| format!("value_scale holds {s}")))
                .transpose()?;
            Ok(FactRow {
                journal_seq: required(c[0][i], "journal_seq")? as u64,
                record_in_frame: required(c[1][i], "record_in_frame")? as u32,
                received_at_ns: required(c[2][i], "received_at")?,
                exchange_time_ns: c[3][i],
                instrument: Arc::from(required(t[0][i].as_deref(), "instrument")?),
                kind: value_of(&KINDS, kind, "kind")?,
                side,
                trade_id: t[3][i].clone(),
                price_scaled: c[4][i],
                quantity_scaled: c[5][i],
                value_scaled: c[6][i],
                value_scale,
                next_funding_ns: c[8][i],
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/facts_tests.rs"]
mod tests;
