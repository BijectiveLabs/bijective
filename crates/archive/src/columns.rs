// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Rows to columns and back. The inverse is exact: a row read back equals the row written,
//! and a column set that cannot be read back into rows is refused with the column named.

use std::sync::Arc;

use bijective_model::market::BookSide;

use crate::rows::{BookRow, BookUpdate, GapKind, GapRow, LevelAction, LevelRow};

/// One column's physical values. `defs` is present for an optional column, one entry per row,
/// with values only for the rows where it is 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    I64 { values: Vec<i64>, defs: Option<Vec<i16>> },
    I32 { values: Vec<i32>, defs: Option<Vec<i16>> },
    Text { values: Vec<String>, defs: Option<Vec<i16>> },
}

impl Column {
    pub fn defs(&self) -> Option<&[i16]> {
        match self {
            Self::I64 { defs, .. } | Self::I32 { defs, .. } | Self::Text { defs, .. } => {
                defs.as_deref()
            }
        }
    }
}

/// Builds an optional column from per-row values.
pub(crate) fn optional<T>(rows: impl Iterator<Item = Option<T>>) -> (Vec<T>, Vec<i16>) {
    let mut values = Vec::new();
    let mut defs = Vec::new();
    for v in rows {
        match v {
            Some(v) => {
                values.push(v);
                defs.push(1);
            }
            None => defs.push(0),
        }
    }
    (values, defs)
}

pub(crate) fn opt_i64(rows: impl Iterator<Item = Option<i64>>) -> Column {
    let (values, defs) = optional(rows);
    Column::I64 { values, defs: Some(defs) }
}

fn opt_text(rows: impl Iterator<Item = Option<&'static str>>) -> Column {
    let (values, defs) = optional(rows.map(|v| v.map(str::to_string)));
    Column::Text { values, defs: Some(defs) }
}

fn side_name(side: BookSide) -> &'static str {
    match side {
        BookSide::Bid => "bid",
        BookSide::Ask => "ask",
    }
}

fn update_name(update: BookUpdate) -> &'static str {
    match update {
        BookUpdate::Snapshot => "snapshot",
        BookUpdate::Delta => "delta",
    }
}

fn action_name(action: LevelAction) -> &'static str {
    match action {
        LevelAction::Set => "set",
        LevelAction::Remove => "remove",
    }
}

/// Unsigned figures are stored as their bit pattern, as the unsigned integer annotation defines.
pub fn book_columns(rows: &[BookRow]) -> Vec<Column> {
    let level = |r: &BookRow| r.level;
    vec![
        Column::I64 { values: rows.iter().map(|r| r.journal_seq as i64).collect(), defs: None },
        Column::I32 { values: rows.iter().map(|r| r.record_in_frame as i32).collect(), defs: None },
        Column::I64 { values: rows.iter().map(|r| r.received_at_ns).collect(), defs: None },
        opt_i64(rows.iter().map(|r| r.exchange_time_ns)),
        Column::Text {
            values: rows.iter().map(|r| r.instrument.to_string()).collect(),
            defs: None,
        },
        opt_i64(rows.iter().map(|r| r.sequence.map(|s| s as i64))),
        Column::Text {
            values: rows.iter().map(|r| update_name(r.update).to_string()).collect(),
            defs: None,
        },
        {
            let (values, defs) = optional(rows.iter().map(|r| level(r).map(|l| l.index as i32)));
            Column::I32 { values, defs: Some(defs) }
        },
        opt_text(rows.iter().map(|r| level(r).map(|l| side_name(l.side)))),
        opt_text(rows.iter().map(|r| level(r).map(|l| action_name(l.action)))),
        opt_i64(rows.iter().map(|r| level(r).map(|l| l.price_scaled))),
        opt_i64(rows.iter().map(|r| level(r).and_then(|l| l.quantity_scaled))),
    ]
}

pub fn gap_columns(rows: &[GapRow]) -> Vec<Column> {
    vec![
        Column::I64 { values: rows.iter().map(|r| r.journal_seq as i64).collect(), defs: None },
        Column::I64 { values: rows.iter().map(|r| r.at_ns).collect(), defs: None },
        Column::Text {
            values: rows.iter().map(|r| r.kind.name().to_string()).collect(),
            defs: None,
        },
        Column::Text { values: rows.iter().map(|r| r.detail.clone()).collect(), defs: None },
    ]
}

/// A column expanded back to one entry per row. `None` where the row holds no value.
fn expand<T: Clone>(
    values: &[T],
    defs: Option<&[i16]>,
    rows: usize,
) -> Result<Vec<Option<T>>, String> {
    let Some(defs) = defs else {
        if values.len() != rows {
            return Err(format!("{} values for {rows} rows", values.len()));
        }
        return Ok(values.iter().cloned().map(Some).collect());
    };
    if defs.len() != rows {
        return Err(format!("{} levels for {rows} rows", defs.len()));
    }
    let mut next = values.iter();
    let out: Vec<Option<T>> =
        defs.iter().map(|d| if *d == 1 { next.next().cloned() } else { None }).collect();
    if next.next().is_some() || out.iter().zip(defs).any(|(v, d)| *d == 1 && v.is_none()) {
        return Err("values and levels disagree".into());
    }
    Ok(out)
}

pub(crate) fn ints(column: &Column, rows: usize, name: &str) -> Result<Vec<Option<i64>>, String> {
    match column {
        Column::I64 { values, defs } => expand(values, defs.as_deref(), rows),
        Column::I32 { values, defs } => {
            let wide: Vec<i64> = values.iter().map(|v| i64::from(*v)).collect();
            expand(&wide, defs.as_deref(), rows)
        }
        Column::Text { .. } => Err(format!("{name} is text, not an integer")),
    }
    .map_err(|e| format!("{name}: {e}"))
}

pub(crate) fn texts(
    column: &Column,
    rows: usize,
    name: &str,
) -> Result<Vec<Option<String>>, String> {
    match column {
        Column::Text { values, defs } => expand(values, defs.as_deref(), rows),
        _ => Err("not text".into()),
    }
    .map_err(|e| format!("{name}: {e}"))
}

pub(crate) fn rows_in(columns: &[Column]) -> usize {
    columns.first().map_or(0, |c| match c {
        Column::I64 { values, defs } => defs.as_ref().map_or(values.len(), Vec::len),
        Column::I32 { values, defs } => defs.as_ref().map_or(values.len(), Vec::len),
        Column::Text { values, defs } => defs.as_ref().map_or(values.len(), Vec::len),
    })
}

pub(crate) fn required<T>(v: Option<T>, name: &str) -> Result<T, String> {
    v.ok_or_else(|| format!("{name} is absent in a required column"))
}

fn named<T: Copy>(
    name: Option<&str>,
    table: &[(&str, T)],
    column: &str,
) -> Result<Option<T>, String> {
    let Some(name) = name else { return Ok(None) };
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| Some(*v))
        .ok_or_else(|| format!("{column} holds {name}"))
}

pub fn book_rows_from(columns: &[Column]) -> Result<Vec<BookRow>, String> {
    if columns.len() != 12 {
        return Err(format!("{} columns, the book table has 12", columns.len()));
    }
    let n = rows_in(columns);
    let c: Vec<Vec<Option<i64>>> = [0, 1, 2, 3, 5, 7, 10, 11]
        .iter()
        .map(|i| ints(&columns[*i], n, "integer column"))
        .collect::<Result<_, _>>()?;
    let t: Vec<Vec<Option<String>>> = [4, 6, 8, 9]
        .iter()
        .map(|i| texts(&columns[*i], n, "text column"))
        .collect::<Result<_, _>>()?;
    (0..n).map(|i| book_row(&c, &t, i)).collect()
}

fn book_row(
    c: &[Vec<Option<i64>>],
    t: &[Vec<Option<String>>],
    i: usize,
) -> Result<BookRow, String> {
    let sides = [("bid", BookSide::Bid), ("ask", BookSide::Ask)];
    let actions = [("set", LevelAction::Set), ("remove", LevelAction::Remove)];
    let updates = [("snapshot", BookUpdate::Snapshot), ("delta", BookUpdate::Delta)];
    let side = named(t[2][i].as_deref(), &sides, "side")?;
    let action = named(t[3][i].as_deref(), &actions, "action")?;
    let level = match (c[5][i], side, action, c[6][i]) {
        (None, None, None, None) => None,
        (Some(index), Some(side), Some(action), Some(price)) => Some(LevelRow {
            index: index as u32,
            side,
            action,
            price_scaled: price,
            quantity_scaled: c[7][i],
        }),
        _ => return Err(format!("row {i} holds part of a level")),
    };
    Ok(BookRow {
        journal_seq: required(c[0][i], "journal_seq")? as u64,
        record_in_frame: required(c[1][i], "record_in_frame")? as u32,
        received_at_ns: required(c[2][i], "received_at")?,
        exchange_time_ns: c[3][i],
        instrument: Arc::from(required(t[0][i].as_deref(), "instrument")?),
        sequence: c[4][i].map(|s| s as u64),
        update: required(named(t[1][i].as_deref(), &updates, "update")?, "update")?,
        level,
    })
}

pub fn gap_rows_from(columns: &[Column]) -> Result<Vec<GapRow>, String> {
    if columns.len() != 4 {
        return Err(format!("{} columns, the gaps table has 4", columns.len()));
    }
    let n = rows_in(columns);
    let seq = ints(&columns[0], n, "journal_seq")?;
    let at = ints(&columns[1], n, "at")?;
    let kind = texts(&columns[2], n, "kind")?;
    let detail = texts(&columns[3], n, "detail")?;
    (0..n)
        .map(|i| {
            let name = required(kind[i].as_deref(), "kind")?;
            Ok(GapRow {
                journal_seq: required(seq[i], "journal_seq")? as u64,
                at_ns: required(at[i], "at")?,
                kind: GapKind::from_name(name).ok_or_else(|| format!("kind holds {name}"))?,
                detail: required(detail[i].clone(), "detail")?,
            })
        })
        .collect()
}
