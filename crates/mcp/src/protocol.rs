// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The Model Context Protocol as this server speaks it: JSON-RPC 2.0, one message per line on
//! standard input and output, and the methods a tool server needs (`initialize`, `ping`,
//! `tools/list`, `tools/call`). Written over `serde_json` (decision 0020, item 5).

use serde_json::{Value, json};

use crate::tools::{self, Context};

/// Protocol revisions this server answers in, newest first. A client asking for one of them is
/// answered in it, any other is offered the newest. The methods used here are the same in each.
pub const REVISIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The reply to one line, or `None` for a notification, which is never answered.
#[must_use]
pub fn reply(ctx: &Context, line: &str) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(&Value::Null, PARSE_ERROR, &format!("not JSON: {e}"))),
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response or a malformed message. Only a request with an id can be answered.
        return message.get("id").map(|id| error(id, INVALID_REQUEST, "a request names a method"));
    };
    let id = message.get("id")?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => result(id, &initialize(&params)),
        "ping" => result(id, &json!({})),
        "tools/list" => result(id, &json!({ "tools": tools::definitions(ctx) })),
        "tools/call" => match call(ctx, &params) {
            Ok(answer) => result(id, &answer),
            Err(reason) => error(id, INVALID_PARAMS, &reason),
        },
        other => error(id, METHOD_NOT_FOUND, &format!("{other} is not a method of this server")),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let revision = asked.filter(|a| REVISIONS.contains(a)).unwrap_or(REVISIONS[0]);
    json!({
        "protocolVersion": revision,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "bijective", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Bijective research tools. Call `platform` first. Strategies run sandboxed, and orders are an operator's act: no tool here places one.",
    })
}

fn call(ctx: &Context, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call names a tool".to_string())?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let answer = tools::call(ctx, name, &args);
    Ok(json!({
        "content": [{ "type": "text", "text": answer.text }],
        "isError": answer.is_error,
    }))
}

fn result(id: &Value, value: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string()
}

fn error(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
#[path = "../tests/unit/protocol_tests.rs"]
mod tests;
