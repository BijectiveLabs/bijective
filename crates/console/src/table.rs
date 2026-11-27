// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A panel as a table a program can read: the open format the console serves under `/api`.
//!
//! Integers travel as decimal strings, because a nanosecond time or a scaled amount is past the
//! 2^53 a JSON number keeps exact in a browser. Each column names its unit and, where the unit
//! fixes one, the scale that turns the integer into the decimal it stands for. Absent is `null`,
//! never zero. The same fold renders the page and the table, so the two cannot disagree.

use crate::panel::{Level, Signal};

/// The format's version, in every document. A change that breaks a reader raises it.
pub const FORMAT: u32 = 1;

/// Decimals of a money figure, as `markup::money` reads it.
pub const MONEY_SCALE: u8 = 8;

/// Decimals of a ratio, as `Ratio::ONE` declares it.
pub const RATIO_SCALE: u8 = 4;

/// Decimals of a span: nanoseconds read as milliseconds.
pub const SPAN_SCALE: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Text,
    Flag,
    /// UTC nanoseconds since the epoch.
    Nanoseconds,
    /// The book's settlement currency, at `MONEY_SCALE`.
    Money,
    /// A ratio at `RATIO_SCALE`.
    Ratio,
    /// At the instrument's own price or quantity scale, which the row does not carry.
    Instrument,
    /// A whole number of things: answers, reconnects, positions in a stream.
    Count,
    /// A duration in nanoseconds. At its scale, milliseconds.
    Span,
    /// An exact decimal written out in full, at the scale its source declared for that row.
    Decimal,
}

impl Unit {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Flag => "flag",
            Self::Nanoseconds => "ns",
            Self::Money => "money",
            Self::Ratio => "ratio",
            Self::Instrument => "instrument",
            Self::Count => "count",
            Self::Span => "span",
            Self::Decimal => "decimal",
        }
    }

    /// The scale the unit fixes. `None` where it fixes none or the row does not carry it.
    #[must_use]
    pub fn scale(self) -> Option<u8> {
        match self {
            Self::Money => Some(MONEY_SCALE),
            Self::Ratio => Some(RATIO_SCALE),
            Self::Count => Some(0),
            Self::Span => Some(SPAN_SCALE),
            Self::Text | Self::Flag | Self::Nanoseconds | Self::Instrument | Self::Decimal => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Text(String),
    Flag(bool),
    Integer(i128),
    Absent,
}

impl Cell {
    #[must_use]
    pub fn text(raw: impl Into<String>) -> Self {
        Self::Text(raw.into())
    }

    #[must_use]
    pub fn integer(value: Option<i128>) -> Self {
        value.map_or(Self::Absent, Self::Integer)
    }

    #[must_use]
    pub fn optional_text(value: Option<&str>) -> Self {
        value.map_or(Self::Absent, Self::text)
    }

    #[must_use]
    pub fn count(value: usize) -> Self {
        Self::Integer(i128::try_from(value).unwrap_or(i128::MAX))
    }

    /// A `Decimal` cell. Absent unless both the figure and its scale are known.
    #[must_use]
    pub fn decimal(scaled: Option<i128>, scale: Option<u8>) -> Self {
        match (scaled, scale) {
            (Some(value), Some(scale)) => Self::Text(crate::markup::decimal(value, scale)),
            _ => Self::Absent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<(&'static str, Unit)>,
    /// Invariant: every row has one cell per column, in column order.
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    #[must_use]
    pub fn new(columns: &[(&'static str, Unit)]) -> Self {
        Self { columns: columns.to_vec(), rows: Vec::new() }
    }

    /// The table as one line of JSON. The same table always gives the same bytes.
    #[must_use]
    pub fn json(&self, panel: &str, title: &str) -> String {
        let mut out = format!("{{\"format\":{FORMAT},\"panel\":");
        quoted(&mut out, panel);
        out.push_str(",\"title\":");
        quoted(&mut out, title);
        out.push_str(",\"columns\":[");
        for (i, (name, unit)) in self.columns.iter().enumerate() {
            comma(&mut out, i);
            out.push_str("{\"name\":");
            quoted(&mut out, name);
            out.push_str(&format!(",\"unit\":\"{}\",\"scale\":", unit.name()));
            match unit.scale() {
                Some(scale) => out.push_str(&scale.to_string()),
                None => out.push_str("null"),
            }
            out.push('}');
        }
        out.push_str("],\"rows\":[");
        for (i, row) in self.rows.iter().enumerate() {
            comma(&mut out, i);
            out.push('[');
            for (j, cell) in row.iter().enumerate() {
                comma(&mut out, j);
                cell_json(&mut out, cell);
            }
            out.push(']');
        }
        out.push_str("]}");
        out
    }
}

/// The strip as a table, so a program sees what every page shows on top.
#[must_use]
pub fn strip(signals: &[Signal]) -> Table {
    let mut table =
        Table::new(&[("label", Unit::Text), ("value", Unit::Text), ("level", Unit::Text)]);
    for signal in signals {
        let level = match signal.level {
            Level::Normal => "normal",
            Level::Attention => "attention",
            Level::Alarm => "alarm",
        };
        table.rows.push(vec![
            Cell::text(signal.label),
            Cell::text(signal.value.clone()),
            Cell::text(level),
        ]);
    }
    table
}

/// The address segment of a panel: its title in lower case, every other character a dash.
#[must_use]
pub fn slug(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect()
}

fn comma(out: &mut String, index: usize) {
    if index > 0 {
        out.push(',');
    }
}

fn cell_json(out: &mut String, cell: &Cell) {
    match cell {
        Cell::Text(raw) => quoted(out, raw),
        Cell::Flag(value) => out.push_str(if *value { "true" } else { "false" }),
        Cell::Integer(value) => out.push_str(&format!("\"{value}\"")),
        Cell::Absent => out.push_str("null"),
    }
}

/// A JSON string. Venue text is not ours: every control character is escaped, so a document
/// is always one line and an event stream frame cannot be split by what it carries.
pub fn quoted(out: &mut String, raw: &str) {
    out.push('"');
    for c in raw.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
#[path = "../tests/unit/table_tests.rs"]
mod tests;
