// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue actions as one tree rendered twice: msgpack (signed) and JSON (sent). A disagreement
//! between the two reads as a missing account. Msgpack order is load-bearing.

use bijective_venue::normalise::{Scale, render_scaled};
use rmp::encode;

/// Maps keep pairs in a vector: a sorting or hashing container would change the signature.
pub enum Value {
    Str(String),
    Int(u64),
    Bool(bool),
    Map(Vec<(&'static str, Value)>),
    Array(Vec<Value>),
}

impl Value {
    fn str(text: impl Into<String>) -> Self {
        Value::Str(text.into())
    }
}

#[must_use]
pub fn pack(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, value);
    out
}

fn write(out: &mut Vec<u8>, value: &Value) {
    // Writing into a Vec cannot fail, so the writer's Result is discarded.
    match value {
        Value::Str(text) => {
            let _ = encode::write_str(out, text);
        }
        Value::Int(number) => {
            let _ = encode::write_uint(out, *number);
        }
        Value::Bool(flag) => {
            let _ = encode::write_bool(out, *flag);
        }
        Value::Map(pairs) => {
            let _ = encode::write_map_len(out, pairs.len() as u32);
            for (key, item) in pairs {
                let _ = encode::write_str(out, key);
                write(out, item);
            }
        }
        Value::Array(items) => {
            let _ = encode::write_array_len(out, items.len() as u32);
            for item in items {
                write(out, item);
            }
        }
    }
}

#[must_use]
pub fn to_json(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value);
    out
}

fn write_json(out: &mut String, value: &Value) {
    match value {
        Value::Str(text) => {
            out.push('"');
            for character in text.chars() {
                match character {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Map(pairs) => {
            out.push('{');
            for (index, (key, item)) in pairs.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json(out, &Value::Str((*key).to_string()));
                out.push(':');
                write_json(out, item);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json(out, item);
            }
            out.push(']');
        }
    }
}

/// This venue refuses trailing zeroes and the other keeps them, so trimming stays here.
#[must_use]
pub fn wire_decimal(value_scaled: i128, scale: Scale) -> String {
    let rendered = render_scaled(value_scaled, scale);
    if !rendered.contains('.') {
        return rendered;
    }
    let trimmed = rendered.trim_end_matches('0');
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    match trimmed {
        // All zero: a bare minus or empty string is unreadable to the venue.
        "" | "-" | "-0" => "0".to_string(),
        value => value.to_string(),
    }
}

/// One value: the contract carries no time-in-force. There is no market order here, only an
/// IOC limit priced away from the book, so a submission with no price is refused.
const TIME_IN_FORCE: &str = "Gtc";

pub struct Placement<'a> {
    /// Position in the venue's own list, which differs per environment.
    pub asset: u64,
    pub buy: bool,
    pub price: String,
    pub size: String,
    pub reduce_only: bool,
    pub client_id: &'a str,
}

#[must_use]
pub fn order_action(placements: &[Placement<'_>]) -> Value {
    let orders = placements
        .iter()
        .map(|placement| {
            Value::Map(vec![
                ("a", Value::Int(placement.asset)),
                ("b", Value::Bool(placement.buy)),
                ("p", Value::str(placement.price.clone())),
                ("s", Value::str(placement.size.clone())),
                ("r", Value::Bool(placement.reduce_only)),
                (
                    "t",
                    Value::Map(vec![(
                        "limit",
                        Value::Map(vec![("tif", Value::str(TIME_IN_FORCE))]),
                    )]),
                ),
                ("c", Value::str(placement.client_id)),
            ])
        })
        .collect();

    Value::Map(vec![
        ("type", Value::str("order")),
        ("orders", Value::Array(orders)),
        // No grouping: one order per decision.
        ("grouping", Value::str("na")),
    ])
}

/// By our own id, never the venue's: the client id survives a modify, the order id does not.
#[must_use]
pub fn cancel_action(asset: u64, client_id: &str) -> Value {
    Value::Map(vec![
        ("type", Value::str("cancelByCloid")),
        (
            "cancels",
            Value::Array(vec![Value::Map(vec![
                ("asset", Value::Int(asset)),
                ("cloid", Value::str(client_id)),
            ])]),
        ),
    ])
}

// Flat file: a submodule would otherwise be looked up in a directory this crate lacks.
#[cfg(test)]
#[path = "../tests/unit/wire_tests.rs"]
mod wire_tests;
