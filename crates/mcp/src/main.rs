// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! `bijective-mcp --workspace <dir> [--allow-backtest] [--bound-seconds <n>]`
//!
//! Reads MCP messages from standard input, one per line, and writes each reply as one line on
//! standard output. Diagnostics go to standard error, so they never corrupt the protocol stream.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use bijective_mcp::{Context, Workspace, protocol};

const PROGRAM: &str = "bijective-mcp";

/// Long enough for a backtest over a day of one instrument, short enough that a stuck program
/// returns the agent's turn.
const DEFAULT_BOUND_SECONDS: u64 = 600;

fn main() -> std::process::ExitCode {
    let ctx = match context(std::env::args().skip(1)) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            eprintln!(
                "usage: {PROGRAM} --workspace <dir> [--allow-backtest] [--bound-seconds <n>]"
            );
            return std::process::ExitCode::from(2);
        }
    };
    eprintln!(
        "{PROGRAM}: serving {} (backtests {})",
        ctx.workspace.root().display(),
        if ctx.allow_backtest { "allowed" } else { "not allowed" }
    );

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = protocol::reply(&ctx, &line) {
            let written = writeln!(stdout, "{reply}").and_then(|()| stdout.flush());
            if written.is_err() {
                break;
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

fn context(mut args: impl Iterator<Item = String>) -> Result<Context, String> {
    let mut workspace: Option<PathBuf> = None;
    let mut allow_backtest = false;
    let mut bound_seconds = DEFAULT_BOUND_SECONDS;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--workspace" => workspace = args.next().map(PathBuf::from),
            "--allow-backtest" => allow_backtest = true,
            "--bound-seconds" => {
                let given = args.next().ok_or("--bound-seconds needs a number")?;
                bound_seconds =
                    given.parse().ok().filter(|s| *s > 0).ok_or_else(|| {
                        format!("--bound-seconds {given} is not a positive number")
                    })?;
            }
            other => return Err(format!("{other} is not an argument of this program")),
        }
    }
    let root = workspace.ok_or("--workspace is required: the one directory the agent works in")?;
    Ok(Context {
        workspace: Workspace::new(&root)?,
        allow_backtest,
        bound: Duration::from_secs(bound_seconds),
    })
}
