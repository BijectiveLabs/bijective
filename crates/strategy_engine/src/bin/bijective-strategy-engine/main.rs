// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The strategy engine supervisor. Holds no credential and opens no venue connection. Ready only
//! when the host can bound a run. There is no degraded mode.

mod bounded;
mod live;
mod markets;
mod run;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use bijective_common::readiness::{Health, Surface};
use bijective_strategy_engine::runs::{Exchange, Stage};
use bijective_strategy_engine::{Behind, Startup, assess};

use crate::bounded::{Bounded, drive};
use crate::live::{Session, live};
use crate::run::now_ns;

const PROGRAM: &str = "strategy-engine";
const HEARTBEAT: Duration = Duration::from_secs(5);

/// Command-line arguments. Every flag takes one value.
#[derive(Default)]
struct Args {
    readiness: Option<PathBuf>,
    cgroup_root: Option<PathBuf>,
    drill_seconds: Option<u64>,
    store_root: Option<PathBuf>,
    /// Repeated for a set: every strategy in it is sent the same window.
    strategies: Vec<PathBuf>,
    /// Repeated for a run that reads several venues, each with its own `--to-position`, in order.
    market_streams: Vec<String>,
    to_positions: Vec<u64>,
    stage: Option<Stage>,
    exchange: Option<Exchange>,
    behind: Option<usize>,
    python_path: Option<String>,
    name: Option<String>,
}

/// Parses arguments. The `Err` is the message for an exit code 2. Stage and exchange are
/// declared, never defaulted: a session at an unset stage would reach a venue no configuration selected.
fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next();
        match (flag.as_str(), value) {
            ("--readiness", Some(v)) => args.readiness = Some(PathBuf::from(v)),
            ("--cgroup-root", Some(v)) => args.cgroup_root = Some(PathBuf::from(v)),
            ("--drill-seconds", Some(v)) => args.drill_seconds = v.parse().ok(),
            ("--store", Some(v)) => args.store_root = Some(PathBuf::from(v)),
            ("--strategy", Some(v)) => {
                let path = PathBuf::from(v);
                // Refused, not merged: the same program twice is one experiment counted twice.
                if args.strategies.contains(&path) {
                    return Err(format!("--strategy {} is given twice", path.display()));
                }
                args.strategies.push(path);
            }
            ("--market", Some(v)) => args.market_streams.push(v),
            ("--exchange", Some(v)) => {
                args.exchange = Some(
                    Exchange::parse(&v)
                        .ok_or(format!("--exchange is whole or stepwise, not {v}"))?,
                );
            }
            // The declared name is the strategy's identity, so moving its file keeps its history.
            ("--name", Some(v)) => args.name = Some(v),
            // The one environment variable a run may receive. Any other name could carry a credential.
            ("--python-path", Some(v)) => args.python_path = Some(v),
            ("--events-behind", Some(v)) => match v.parse::<usize>() {
                Ok(n) if n > 0 => args.behind = Some(n),
                _ => return Err(format!("--events-behind wants a count above zero, not {v}")),
            },
            ("--stage", Some(v)) => {
                args.stage =
                    Some(Stage::parse(&v).ok_or(format!("--stage is paper or live, not {v}"))?);
            }
            // Parsed, not ignored: an unread window is a run over a different stretch of history.
            ("--to-position", Some(v)) => {
                args.to_positions.push(
                    v.parse::<u64>()
                        .map_err(|_| format!("--to-position wants a store position, not {v}"))?,
                );
            }
            (other, _) => return Err(format!("unusable argument {other}")),
        }
    }
    Ok(args)
}

fn usage(detail: &str) -> ExitCode {
    eprintln!("{PROGRAM}: {detail}");
    ExitCode::from(2)
}

fn fail(surface: &Surface, detail: &str) -> ExitCode {
    surface.report_or_warn(Health::Failed, detail, now_ns());
    eprintln!("{PROGRAM}: {detail}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(detail) => return usage(&detail),
    };
    // Required: a default cgroup root would bound runs in a cgroup subtree that was never delegated.
    let (Some(readiness), Some(cgroup_root)) = (args.readiness.clone(), args.cgroup_root.clone())
    else {
        return usage("--readiness and --cgroup-root are required");
    };
    let surface = Surface::new(&readiness, PROGRAM);
    if let Err(e) = surface.report(Health::Starting, "checking the host can bound a run", now_ns())
    {
        return usage(&format!("the readiness surface could not be written: {e}"));
    }
    // A supervisor that cannot write the run ledger cannot record a run.
    if let Some(store) = args.store_root.as_deref()
        && let Err(e) = bijective_event_store::store::writable(store)
    {
        return fail(
            &surface,
            &format!("the store at {} cannot be written: {e:?}", store.display()),
        );
    }
    let detail = match assess(bijective_sandbox::probe(&cgroup_root)) {
        Startup::Ready { detail } => {
            surface.report_or_warn(Health::Ready, &detail, now_ns());
            detail
        }
        Startup::Refused { detail } => return fail(&surface, &detail),
    };

    match (&args.store_root, args.strategies.is_empty(), args.market_streams.is_empty()) {
        (Some(_), false, false) => run_once(&surface, &cgroup_root, args),
        (None, true, true) => idle(&surface, &detail, args.drill_seconds),
        _ => usage("a run needs --store, --strategy and --market, or none of them"),
    }
}

/// Bounded runs (`--to-position` given) or a live session, of every strategy given.
fn run_once(surface: &Surface, cgroup_root: &std::path::Path, args: Args) -> ExitCode {
    let programs = args.strategies;
    let streams = args.market_streams;
    let Some(store) = args.store_root else {
        return usage("a run needs --store, --strategy and --market, or none of them");
    };
    // Declared, not inherited, and part of the run's identity.
    let mut environment = BTreeMap::new();
    if let Some(path) = args.python_path {
        environment.insert("PYTHONPATH".to_string(), path);
    }
    let bounded = !args.to_positions.is_empty();
    let outcome = match (bounded, args.exchange, args.stage) {
        // One end per market, in the order the markets were named: an end left out is a window
        // nobody declared.
        (true, _, _) if args.to_positions.len() != streams.len() => Err(format!(
            "{} market(s) and {} --to-position(s): each market needs its own end",
            streams.len(),
            args.to_positions.len()
        )),
        (true, Some(exchange), _) => {
            let named: Vec<(String, u64)> =
                streams.iter().cloned().zip(args.to_positions.iter().copied()).collect();
            drive(
                surface,
                cgroup_root,
                &store,
                &programs,
                &named,
                Bounded { exchange, environment, called: args.name },
            )
        }
        (true, None, _) => {
            Err("a bounded run needs --exchange whole or --exchange stepwise".to_string())
        }
        (false, _, Some(stage)) => {
            let tolerated = match args.behind {
                Some(n) => Behind::Declared(n),
                None => Behind::NotDeclared(bijective_sandbox::live::QUEUED_BY_DEFAULT),
            };
            live(
                surface,
                cgroup_root,
                &store,
                &programs,
                &streams,
                Session {
                    stage,
                    seconds: args.drill_seconds,
                    tolerated,
                    environment,
                    called: args.name,
                },
            )
        }
        (false, _, None) => Err("a live session needs --stage paper or --stage live".to_string()),
    };
    match outcome {
        Ok(detail) => {
            surface.report_or_warn(Health::Ready, &detail, now_ns());
            println!("{PROGRAM}: {detail}");
            ExitCode::SUCCESS
        }
        Err(detail) => fail(surface, &detail),
    }
}

/// No run requested: report readiness every heartbeat until `--drill-seconds` elapses.
fn idle(surface: &Surface, detail: &str, drill_seconds: Option<u64>) -> ExitCode {
    let mut elapsed = 0u64;
    loop {
        if drill_seconds.is_some_and(|d| elapsed >= d) {
            return ExitCode::SUCCESS;
        }
        sleep(HEARTBEAT);
        elapsed += HEARTBEAT.as_secs();
        surface.report_or_warn(Health::Ready, detail, now_ns());
    }
}
