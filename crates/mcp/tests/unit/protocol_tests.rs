// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The JSON-RPC exchange a client depends on: initialize, list, call, and an error for anything else.

use std::time::Duration;

use serde_json::Value;

use super::{REVISIONS, reply};
use crate::tools::Context;
use crate::workspace::Workspace;

fn context() -> Context {
    let dir = std::env::temp_dir().join(format!("bl-mcp-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Context {
        workspace: Workspace::new(&dir).unwrap(),
        allow_backtest: false,
        bound: Duration::from_secs(5),
    }
}

fn ask(line: &str) -> Value {
    serde_json::from_str(&reply(&context(), line).expect("a reply")).unwrap()
}

#[test]
fn initialize_answers_in_the_revision_asked_for() {
    let answer = ask(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
    );
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
    assert!(answer["result"]["capabilities"]["tools"].is_object());
}

#[test]
fn an_unknown_revision_is_offered_the_newest() {
    let answer = ask(
        r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
    );
    assert_eq!(answer["result"]["protocolVersion"], REVISIONS[0]);
}

#[test]
fn a_notification_is_not_answered() {
    assert!(
        reply(&context(), r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none()
    );
}

#[test]
fn tools_are_listed_with_their_schemas() {
    let answer = ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = answer["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
}

#[test]
fn a_call_answers_with_text_content() {
    let answer = ask(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"platform","arguments":{}}}"#,
    );
    assert_eq!(answer["result"]["content"][0]["type"], "text");
    assert_eq!(answer["result"]["isError"], false);
}

/// A tool that fails is a result with `isError`, so the agent reads why and can try again.
#[test]
fn a_failing_tool_is_a_result_not_a_protocol_error() {
    let answer = ask(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nothing","arguments":{}}}"#,
    );
    assert_eq!(answer["result"]["isError"], true);
}

#[test]
fn an_unknown_method_is_a_protocol_error() {
    let answer = ask(r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#);
    assert_eq!(answer["error"]["code"], -32601);
}

#[test]
fn a_line_that_is_not_json_is_a_parse_error() {
    let answer = ask("not json");
    assert_eq!(answer["error"]["code"], -32700);
}
