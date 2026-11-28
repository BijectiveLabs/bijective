// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What an agent is offered, and what it is refused before any program runs.

use std::time::Duration;

use serde_json::json;

use super::{Context, call, definitions};
use crate::workspace::Workspace;

fn context(allow_backtest: bool) -> Context {
    let dir = std::env::temp_dir().join(format!("bl-mcp-tools-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Context {
        workspace: Workspace::new(&dir).unwrap(),
        allow_backtest,
        bound: Duration::from_secs(5),
    }
}

fn names(ctx: &Context) -> Vec<String> {
    definitions(ctx).iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn read_only_by_default() {
    let offered = names(&context(false));
    assert!(offered.contains(&"panel".to_string()));
    assert!(!offered.contains(&"backtest".to_string()));
    assert!(!offered.contains(&"new_strategy".to_string()));
}

#[test]
fn backtests_are_offered_when_the_operator_allows_them() {
    let offered = names(&context(true));
    assert!(
        offered.contains(&"backtest".to_string()) && offered.contains(&"new_strategy".to_string())
    );
}

/// Asked by name though not listed, a writing tool is still refused.
#[test]
fn a_backtest_asked_for_without_permission_is_refused() {
    let answer = call(&context(false), "backtest", &json!({ "strategy": "alpha.py" }));
    assert!(answer.is_error);
    assert!(answer.text.contains("--allow-backtest"));
}

/// No tool can reach an order, a session or a command, whatever its name.
#[test]
fn no_tool_places_an_order_or_starts_a_session() {
    let offered = names(&context(true));
    for forbidden in ["order", "paper", "live", "issue", "resume", "promote"] {
        assert!(offered.iter().all(|n| !n.contains(forbidden)), "{forbidden} offered: {offered:?}");
    }
    assert!(call(&context(true), "submit_order", &json!({})).is_error);
}

#[test]
fn a_path_out_of_the_workspace_is_refused_before_running() {
    let answer = call(&context(true), "report", &json!({ "store": "../../etc" }));
    assert!(answer.is_error);
    assert!(answer.text.contains("workspace"));
}

#[test]
fn a_panel_name_that_is_not_a_word_is_refused() {
    let answer = call(&context(false), "panel", &json!({ "store": ".", "panel": "../control" }));
    assert!(answer.is_error && answer.text.contains("not a name"));
}

#[test]
fn a_confidence_that_is_not_a_whole_number_is_refused() {
    let args = json!({ "store": ".", "strategy": "alpha.py", "confidence": "1.96" });
    let answer = call(&context(false), "promotion", &args);
    assert!(answer.is_error && answer.text.contains("whole number"));
}

#[test]
fn the_platform_tool_says_what_it_cannot_do() {
    let answer = call(&context(false), "platform", &json!({}));
    assert!(!answer.is_error);
    assert!(answer.text.contains("place an order"));
}
