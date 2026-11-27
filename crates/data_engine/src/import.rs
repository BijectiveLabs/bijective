// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Market history from a client's file, read into the top-of-book records a recording writes
//! (`docs/decisions/0016-market-history-from-files.md`). Pure over lines, so every refusal is
//! tested without a store, and the whole file is read before anything is written.

use std::collections::BTreeMap;

use bijective_model::Sequence;
use bijective_serialization::signal::market::BookUpdate;
use bijective_venue::normalise::{DecimalError, Scale, parse_scaled};
use bijective_venue::profile::{InstrumentCatalogue, VenueSymbol};

/// The fields a row must carry, by their canonical names.
const FIELDS: [&str; 5] = ["time", "bid_price", "bid_quantity", "ask_price", "ask_quantity"];

/// The unit the file's time column counts in, stated by the operator, never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Nanoseconds,
    Microseconds,
    Milliseconds,
    Seconds,
}

impl TimeUnit {
    #[must_use]
    pub fn named(text: &str) -> Option<Self> {
        match text {
            "ns" => Some(Self::Nanoseconds),
            "us" => Some(Self::Microseconds),
            "ms" => Some(Self::Milliseconds),
            "s" => Some(Self::Seconds),
            _ => None,
        }
    }

    fn nanoseconds(self) -> i128 {
        match self {
            Self::Nanoseconds => 1,
            Self::Microseconds => 1_000,
            Self::Milliseconds => 1_000_000,
            Self::Seconds => 1_000_000_000,
        }
    }
}

/// How the file is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A header line naming the columns, then one comma-separated row per line.
    Csv,
    /// One JSON object per line.
    JsonLines,
}

/// How to read a file: its layout, its time unit, the venue's catalogue that scales its values,
/// the file's names for the canonical fields, and the instrument when the file names none.
pub struct Reading<'a> {
    pub format: Format,
    pub unit: TimeUnit,
    pub catalogue: &'a InstrumentCatalogue,
    /// Canonical field to the file's name for it. A field not listed keeps its canonical name.
    pub renamed: BTreeMap<String, String>,
    pub instrument: Option<VenueSymbol>,
}

/// Why a row was refused. Each names the field and what was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowError {
    Missing {
        field: String,
    },
    Unlisted {
        instrument: String,
    },
    Value {
        field: String,
        cause: DecimalError,
    },
    Time {
        found: String,
    },
    Backwards {
        at_ns: i128,
        after_ns: i128,
    },
    /// A quoted CSV field is not read: a comma inside quotes would shift every column after it.
    Quoted,
    /// A JSON number passes through a binary float, which can change its decimal digits.
    Float {
        field: String,
    },
    Unreadable {
        detail: String,
    },
}

/// The first refused row and its line number, counted from 1 as an editor counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub line: usize,
    pub error: RowError,
}

impl core::fmt::Display for Refused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "line {}: ", self.line)?;
        match &self.error {
            RowError::Missing { field } => write!(f, "no {field}"),
            RowError::Unlisted { instrument } => {
                write!(f, "{instrument} is not in the venue's profile")
            }
            RowError::Value { field, cause } => write!(f, "{field}: {cause}"),
            RowError::Time { found } => write!(f, "time {found:?} is not a whole number"),
            RowError::Backwards { at_ns, after_ns } => {
                write!(f, "time {at_ns} ns is before the row above it ({after_ns} ns)")
            }
            RowError::Quoted => write!(f, "quoted fields are not read; export without quotes"),
            RowError::Float { field } => {
                write!(f, "{field} is a JSON number; give prices and quantities as strings")
            }
            RowError::Unreadable { detail } => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for Refused {}

/// Every row of `lines` as a top of book, in file order. The first refused row stops the read.
pub fn read_books<'l>(
    lines: impl Iterator<Item = &'l str>,
    reading: &Reading<'_>,
) -> Result<Vec<BookUpdate>, Refused> {
    let mut books = Vec::new();
    let mut header: Option<Vec<String>> = None;
    let mut last_ns: Option<i128> = None;
    for (index, line) in lines.enumerate() {
        let line_no = index + 1;
        let refused = |error| Refused { line: line_no, error };
        if line.trim().is_empty() {
            continue;
        }
        let fields = match reading.format {
            Format::Csv if header.is_none() => {
                header = Some(line.split(',').map(|c| c.trim().to_string()).collect());
                continue;
            }
            Format::Csv => csv_row(header.as_deref().unwrap_or(&[]), line).map_err(refused)?,
            Format::JsonLines => json_row(line).map_err(refused)?,
        };
        let (book, at_ns) = book_of(&fields, reading).map_err(refused)?;
        if let Some(after_ns) = last_ns.filter(|after| at_ns < *after) {
            return Err(refused(RowError::Backwards { at_ns, after_ns }));
        }
        last_ns = Some(at_ns);
        books.push(book);
    }
    Ok(books)
}

fn csv_row(header: &[String], line: &str) -> Result<BTreeMap<String, String>, RowError> {
    if line.contains('"') {
        return Err(RowError::Quoted);
    }
    Ok(header.iter().cloned().zip(line.split(',').map(|v| v.trim().to_string())).collect())
}

fn json_row(line: &str) -> Result<BTreeMap<String, String>, RowError> {
    let parsed: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(line).map_err(|e| RowError::Unreadable { detail: e.to_string() })?;
    let mut row = BTreeMap::new();
    for (name, value) in parsed {
        let text = match value {
            serde_json::Value::String(text) => text,
            serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            serde_json::Value::Number(_) => return Err(RowError::Float { field: name }),
            _ => continue,
        };
        row.insert(name, text);
    }
    Ok(row)
}

/// One row as a top of book at the instrument's own precision, and its instant in nanoseconds.
fn book_of(
    row: &BTreeMap<String, String>,
    reading: &Reading<'_>,
) -> Result<(BookUpdate, i128), RowError> {
    let field = |canonical: &str| {
        let name = reading.renamed.get(canonical).map_or(canonical, String::as_str);
        row.get(name).filter(|v| !v.is_empty()).ok_or(RowError::Missing { field: name.into() })
    };
    for canonical in FIELDS {
        field(canonical)?;
    }
    let symbol = match &reading.instrument {
        Some(symbol) => symbol.clone(),
        None => VenueSymbol::new(field("instrument")?.as_str()),
    };
    let listed = reading
        .catalogue
        .get(&symbol)
        .map_err(|_| RowError::Unlisted { instrument: symbol.to_string() })?;
    let price = scale(listed.precision.price_scale)?;
    let quantity = scale(listed.precision.quantity_scale)?;
    let value = |name: &str, at: Scale| {
        parse_scaled(field(name)?, at)
            .map_err(|cause| RowError::Value { field: name.into(), cause })
    };
    let at_ns = instant(field("time")?, reading.unit)?;
    let book = BookUpdate {
        instrument: listed.id.clone(),
        // A file numbers nothing the venue published. A fabricated sequence would claim continuity.
        seq: Sequence::NotNumbered,
        venue_ts_ms: u64::try_from(at_ns / 1_000_000)
            .map_err(|_| RowError::Time { found: at_ns.to_string() })?,
        received_at_ns: None,
        bid_price: value("bid_price", price)?,
        bid_quantity: value("bid_quantity", quantity)?,
        ask_price: value("ask_price", price)?,
        ask_quantity: value("ask_quantity", quantity)?,
        bid_depth: 1,
        ask_depth: 1,
    };
    Ok((book, at_ns))
}

fn scale(digits: u8) -> Result<Scale, RowError> {
    Scale::new(digits).map_err(|e| RowError::Unreadable { detail: e.to_string() })
}

fn instant(text: &str, unit: TimeUnit) -> Result<i128, RowError> {
    let whole: i128 = text.parse().map_err(|_| RowError::Time { found: text.to_string() })?;
    whole
        .checked_mul(unit.nanoseconds())
        .filter(|ns| *ns >= 0)
        .ok_or(RowError::Time { found: text.to_string() })
}

#[cfg(test)]
#[path = "../tests/unit/import_tests.rs"]
mod tests;
