// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The private stream's frames: authentication, subscription, and the order and execution topics,
//! read through the same row parsers as the REST evidence. Opens nothing.

use bijective_lifecycle::{FillReport, OrderStatusReport};
use serde_json::Value;

use crate::evidence::{Execution, InstrumentScales, RowRefused, execution, order_report};
use crate::sign::sign_realtime;

/// The categorised topics for this adapter's product (V5 private Order and Execution streams).
pub const TOPICS: [&str; 2] = ["order.linear", "execution.linear"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateMessage {
    /// The venue's answer to `auth`. A refusal carries its message.
    Authenticated {
        success: bool,
        detail: String,
    },
    Subscribed {
        success: bool,
        detail: String,
    },
    Pong,
    Orders(Vec<Result<OrderStatusReport, RowRefused>>),
    Executions(Vec<Result<Execution, RowRefused>>),
    /// Valid JSON of a topic this adapter does not consume. Named so a venue change is seen.
    Unrecognised {
        topic: Option<String>,
    },
}

/// `{"op":"auth","args":[key, expires, signature]}`. The expiry must be later than the venue's
/// clock when it arrives (V5 WebSocket connect guide, Authentication).
pub fn auth_frame(api_key: &str, secret: &[u8], expires_ms: u64) -> String {
    serde_json::json!({ "op": "auth", "args": [api_key, expires_ms, sign_realtime(secret, expires_ms)] })
        .to_string()
}

/// How long a pushed fill is kept for the evidence reads. The execution history listed fills 17
/// to 127 s after the venue had pushed them (measured 2026-11-18). A quarter of an hour covers
/// that with room, and a fill dropped sooner is still read from the history, only later.
pub const PUSHED_KEPT_FOR_NS: i128 = 15 * 60 * 1_000_000_000;

/// Bounds the memory a burst of executions can take. The oldest go first.
pub const PUSHED_KEPT_AT_MOST: usize = 1_000;

/// Adds the fills of one execution frame to those held, then drops what is older than
/// `PUSHED_KEPT_FOR_NS` on the wall clock and, past `PUSHED_KEPT_AT_MOST`, the oldest. A refused
/// row adds nothing: the history read judges that record and marks its evidence incomplete.
pub fn retain_pushed(
    held: &mut Vec<FillReport>,
    rows: &[Result<Execution, RowRefused>],
    now_ns: i128,
) {
    for row in rows {
        if let Ok(Execution::Fill(report)) = row {
            held.push(report.as_ref().clone());
        }
    }
    held.retain(|r| now_ns - r.fill.received_at_ns < PUSHED_KEPT_FOR_NS);
    let excess = held.len().saturating_sub(PUSHED_KEPT_AT_MOST);
    held.drain(..excess);
}

pub fn subscribe_frame() -> String {
    serde_json::json!({ "op": "subscribe", "args": TOPICS }).to_string()
}

/// Every row of a topic frame is judged. One refused row does not hide the others.
pub fn parse(
    text: &str,
    scales: &InstrumentScales,
    received_at_ns: i128,
) -> Result<PrivateMessage, RowRefused> {
    let frame: Value = serde_json::from_str(text)
        .map_err(|_| RowRefused { field: "frame", detail: "not JSON".into() })?;
    if let Some(op) = frame.get("op").and_then(Value::as_str) {
        let success = frame.get("success").and_then(Value::as_bool).unwrap_or(false);
        let detail = frame.get("ret_msg").and_then(Value::as_str).unwrap_or("").to_string();
        return Ok(match op {
            "auth" => PrivateMessage::Authenticated { success, detail },
            "subscribe" => PrivateMessage::Subscribed { success, detail },
            "pong" | "ping" => PrivateMessage::Pong,
            other => PrivateMessage::Unrecognised { topic: Some(format!("op {other}")) },
        });
    }
    let topic = frame.get("topic").and_then(Value::as_str);
    let rows = || -> Result<&Vec<Value>, RowRefused> {
        frame
            .get("data")
            .and_then(Value::as_array)
            .ok_or(RowRefused { field: "data", detail: "absent".into() })
    };
    match topic {
        Some(t) if t == "order" || t.starts_with("order.") => {
            Ok(PrivateMessage::Orders(rows()?.iter().map(|r| order_report(r, scales)).collect()))
        }
        Some(t) if t == "execution" || t.starts_with("execution.") => {
            Ok(PrivateMessage::Executions(
                rows()?.iter().map(|r| execution(r, scales, received_at_ns)).collect(),
            ))
        }
        other => Ok(PrivateMessage::Unrecognised { topic: other.map(str::to_string) }),
    }
}

#[cfg(test)]
#[path = "../tests/unit/private_tests.rs"]
mod tests;
