// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The tools an agent is offered, and the program each one runs.
//!
//! A tool that writes (`new_strategy`, `backtest`) is listed only when the operator started the
//! server with `--allow-backtest`. No tool starts a paper or live session, issues a command or
//! writes a proposal stream: those are an operator's acts (decision 0020).

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::panels;
use crate::run::{self, Outcome};
use crate::workspace::Workspace;

/// What every tool call runs under, fixed when the server starts.
#[derive(Debug, Clone)]
pub struct Context {
    pub workspace: Workspace,
    pub allow_backtest: bool,
    /// The longest a program may run before it is ended.
    pub bound: Duration,
}

/// What a tool returns: text for the agent, and whether it is a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub is_error: bool,
}

impl Answer {
    fn refused(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }

    fn from(outcome: Outcome) -> Self {
        Self { is_error: !outcome.succeeded(), text: outcome.output }
    }
}

const MODES: &[&str] = &["live", "paper", "backtest"];

/// The tool list `tools/list` answers, in the order an agent would use them.
#[must_use]
pub fn definitions(ctx: &Context) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "platform",
            "description": "What this server offers: the workspace, the programs it found, and whether backtests may run. Call this first.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "panels",
            "description": "List the console tables a record holds (orders, fills, positions, risk, market data, strategy performance and more).",
            "inputSchema": schema(&[("store", "Record directory in the workspace", true), ("mode", "live, paper or backtest (default live)", false)])
        }),
        json!({
            "name": "panel",
            "description": "Read one console table from a record as JSON, in the console's open table format.",
            "inputSchema": schema(&[("store", "Record directory in the workspace", true), ("panel", "Table name from `panels`", true), ("mode", "live, paper or backtest (default live)", false)])
        }),
        json!({
            "name": "report",
            "description": "The evaluation report of the strategies that ran against a record. With a strategy and a confidence, that strategy's verdict.",
            "inputSchema": schema(&[("store", "Record directory in the workspace", true), ("strategy", "Strategy program path", false), ("confidence", "Critical value in the fixed scale, such as 19600", false)])
        }),
        json!({
            "name": "promotion",
            "description": "How far a strategy has come towards capital, and what stops it going further. Promotion itself is an operator's signed act.",
            "inputSchema": schema(&[("store", "Record directory in the workspace", true), ("strategy", "Strategy program path", true), ("confidence", "Critical value in the fixed scale, such as 19600", true)])
        }),
    ];
    if ctx.allow_backtest {
        tools.push(json!({
            "name": "new_strategy",
            "description": "Write a strategy that already runs, and seed a sample market so a backtest needs no venue. Edit it, then run `backtest`.",
            "inputSchema": schema(&[("path", "Where to write the strategy, such as alpha.py", true)])
        }));
        tools.push(json!({
            "name": "backtest",
            "description": "Run a strategy over a recorded market in the sandbox. With a node configuration, its proposals pass risk evaluation and fill on simulated venues through the order path.",
            "inputSchema": schema(&[("strategy", "Strategy program, or a directory of them", true), ("market", "Market stream to read", false), ("store", "Record directory to read and write", false), ("config", "Node configuration file, to place orders on simulated venues", false)])
        }));
    }
    tools
}

fn schema(fields: &[(&str, &str, bool)]) -> Value {
    let properties: serde_json::Map<String, Value> = fields
        .iter()
        .map(|(name, about, _)| {
            ((*name).to_string(), json!({ "type": "string", "description": about }))
        })
        .collect();
    let required: Vec<&str> = fields.iter().filter(|f| f.2).map(|f| f.0).collect();
    json!({ "type": "object", "properties": properties, "required": required })
}

/// Runs tool `name` with `args`. Every refusal says why, so an agent can correct its call.
#[must_use]
pub fn call(ctx: &Context, name: &str, args: &Value) -> Answer {
    let answered = match name {
        "platform" => Ok(platform(ctx)),
        "panels" => panel_route(ctx, args, None),
        "panel" => text(args, "panel")
            .and_then(|p| table_name(&p))
            .and_then(|p| panel_route(ctx, args, Some(&p))),
        "report" => report(ctx, args),
        "promotion" => promotion(ctx, args),
        "new_strategy" | "backtest" if !ctx.allow_backtest => Err(format!(
            "{name} is not offered: the operator started the server without --allow-backtest"
        )),
        "new_strategy" => new_strategy(ctx, args),
        "backtest" => backtest(ctx, args),
        other => Err(format!("{other} is not a tool of this server")),
    };
    answered.unwrap_or_else(Answer::refused)
}

fn platform(ctx: &Context) -> Answer {
    let programs: Vec<Value> = PROGRAMS
        .iter()
        .map(|p| json!({ "program": p, "found": run::locate(p).map(|f| f.display().to_string()) }))
        .collect();
    let about = json!({
        "server": "bijective-mcp",
        "version": env!("CARGO_PKG_VERSION"),
        "workspace": ctx.workspace.root().display().to_string(),
        "backtests_allowed": ctx.allow_backtest,
        "program_bound_seconds": ctx.bound.as_secs(),
        "programs": programs,
        "cannot": "start a paper or live session, sign a command, read a credential or place an order",
    });
    Answer { text: about.to_string(), is_error: false }
}

const PROGRAMS: &[&str] =
    &["bijective-backtest", "bijective-report", "bijective-promote", "bijective-console"];

fn panel_route(ctx: &Context, args: &Value, panel: Option<&str>) -> Result<Answer, String> {
    let store = path_arg(ctx, args, "store")?;
    let mode = optional(args, "mode")?.unwrap_or_else(|| "live".to_string());
    if !MODES.contains(&mode.as_str()) {
        return Err(format!("mode {mode} is not one of live, paper, backtest"));
    }
    let console = program("bijective-console")?;
    let route = panel.map_or_else(|| "/api/panels".to_string(), |p| format!("/api/panels/{p}"));
    let body = panels::read(&console, &store, &mode, &route)?;
    Ok(Answer { text: run::truncated(body), is_error: false })
}

fn report(ctx: &Context, args: &Value) -> Result<Answer, String> {
    let mut argv: Vec<OsString> = vec![path_arg(ctx, args, "store")?.into()];
    let strategy = optional(args, "strategy")?;
    let confidence = optional(args, "confidence")?;
    match (strategy, confidence) {
        (Some(s), Some(c)) => {
            argv.push(ctx.workspace.resolve(&s)?.into());
            argv.push(digits("confidence", &c)?.into());
        }
        (None, None) => {}
        _ => return Err("strategy and confidence are given together or not at all".into()),
    }
    execute(ctx, "bijective-report", &argv)
}

fn promotion(ctx: &Context, args: &Value) -> Result<Answer, String> {
    let argv: Vec<OsString> = vec![
        path_arg(ctx, args, "store")?.into(),
        path_arg(ctx, args, "strategy")?.into(),
        digits("confidence", &text(args, "confidence")?)?.into(),
    ];
    execute(ctx, "bijective-promote", &argv)
}

fn new_strategy(ctx: &Context, args: &Value) -> Result<Answer, String> {
    let argv: Vec<OsString> = vec!["--new".into(), path_arg(ctx, args, "path")?.into()];
    execute(ctx, "bijective-backtest", &argv)
}

fn backtest(ctx: &Context, args: &Value) -> Result<Answer, String> {
    let mut argv: Vec<OsString> = vec![path_arg(ctx, args, "strategy")?.into()];
    if let Some(market) = optional(args, "market")? {
        argv.push("--market".into());
        argv.push(stream_name(&market)?.into());
    }
    for flag in ["store", "config"] {
        if optional(args, flag)?.is_some() {
            argv.push(format!("--{flag}").into());
            argv.push(path_arg(ctx, args, flag)?.into());
        }
    }
    execute(ctx, "bijective-backtest", &argv)
}

fn execute(ctx: &Context, name: &str, argv: &[OsString]) -> Result<Answer, String> {
    let program = program(name)?;
    run::run(&program, argv, ctx.workspace.root(), ctx.bound).map(Answer::from)
}

fn program(name: &str) -> Result<PathBuf, String> {
    run::locate(name)
        .ok_or_else(|| format!("{name} was found neither beside this server nor on PATH"))
}

fn text(args: &Value, key: &str) -> Result<String, String> {
    optional(args, key)?.ok_or_else(|| format!("{key} is required"))
}

fn optional(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(_) => Err(format!("{key} must be a non-empty string")),
    }
}

fn path_arg(ctx: &Context, args: &Value, key: &str) -> Result<PathBuf, String> {
    ctx.workspace.resolve(&text(args, key)?)
}

fn digits(key: &str, value: &str) -> Result<String, String> {
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        Ok(value.to_string())
    } else {
        Err(format!("{key} must be a whole number in the fixed scale, such as 19600"))
    }
}

/// Table and stream names are words, so an argument can never become a route or a flag.
fn table_name(value: &str) -> Result<String, String> {
    named("panel", value)
}

fn stream_name(value: &str) -> Result<String, String> {
    named("market", value)
}

fn named(key: &str, value: &str) -> Result<String, String> {
    let fine = !value.starts_with('-')
        && !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if fine { Ok(value.to_string()) } else { Err(format!("{key} {value} is not a name")) }
}

#[cfg(test)]
#[path = "../tests/unit/tools_tests.rs"]
mod tests;
