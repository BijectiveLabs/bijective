// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One command for a paper run on a live public market: records the market, runs the strategy
//! in a paper session, and judges, places and fills its proposals on simulated venues.
//! Usage: bijective-paper <strategy> <setup-dir> <stream-endpoint> [--store <dir>] [--minutes <n>]
//! `<setup-dir>` is what `bijective-profile ... --out <dir>` wrote: `profile.json` and
//! `config.json`. No credential is read and no order leaves the node.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::time::{Duration, Instant};

use bijective_strategy_engine::launch::{
    Bounded, beside_me, bounded, library_for_python, say_how_to_bound,
};

const PROGRAM: &str = "bijective-paper";
const USAGE: &str = "usage: bijective-paper <strategy> <setup-dir> <stream-endpoint> [--store <dir>] [--minutes <n>]";
/// The market must have arrived before the strategy starts, or the session waits on nothing.
const FIRST_BOOK_WAIT: Duration = Duration::from_secs(30);
/// The recorder runs a little past the session, so the last pass still reads a fresh book.
const RECORDER_MARGIN_SECONDS: u64 = 15;

/// What was asked for, resolved.
struct Run {
    strategy: PathBuf,
    setup: PathBuf,
    endpoint: String,
    store: PathBuf,
    seconds: u64,
}

/// The venue and its instruments, as the drafted profile names them.
struct Market {
    venue: String,
    symbols: Vec<String>,
}

fn main() -> ExitCode {
    match parse().and_then(|run| launch(&run)) {
        Ok(code) => code,
        Err(detail) => {
            eprintln!("{PROGRAM}: {detail}");
            ExitCode::FAILURE
        }
    }
}

fn parse() -> Result<Run, String> {
    let mut args = std::env::args().skip(1);
    let (Some(strategy), Some(setup), Some(endpoint)) = (args.next(), args.next(), args.next())
    else {
        return Err(USAGE.to_string());
    };
    let mut run = Run {
        strategy: absolute(Path::new(&strategy))?,
        setup: absolute(Path::new(&setup))?,
        endpoint,
        store: PathBuf::from("store"),
        seconds: 600,
    };
    while let Some(flag) = args.next() {
        match (flag.as_str(), args.next()) {
            ("--store", Some(v)) => run.store = PathBuf::from(v),
            ("--minutes", Some(v)) => {
                let minutes: u64 =
                    v.parse().map_err(|_| format!("--minutes wants a count, not {v}"))?;
                run.seconds = minutes.max(1) * 60;
            }
            _ => return Err(USAGE.to_string()),
        }
    }
    std::fs::create_dir_all(run.store.join("run").join("paper"))
        .map_err(|e| format!("{} could not be made: {e}", run.store.display()))?;
    run.store = absolute(&run.store)?;
    Ok(run)
}

/// A relative path resolves elsewhere once a run starts in its own directory.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|e| format!("{} is not there: {e}", path.display()))
}

fn launch(run: &Run) -> Result<ExitCode, String> {
    let market = read_market(&run.setup.join("profile.json"))?;
    let cgroup = match bounded() {
        Ok(Bounded::Under(root)) => root,
        Ok(Bounded::RanInScope(code)) => return Ok(code),
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            say_how_to_bound(PROGRAM);
            return Ok(ExitCode::from(2));
        }
    };
    let stream = format!("market-{}", market.venue);
    println!(
        "paper:    {} on {} {}, {} s",
        run.strategy.display(),
        market.venue,
        market.symbols.join(","),
        run.seconds
    );
    println!("store:    {}", run.store.display());

    let mut recorder = spawn(record(run, &market)?)?;
    if let Err(detail) = first_book(&run.store, &stream) {
        stop(&mut recorder);
        return Err(detail);
    }
    let mut session = match spawn(session(run, &stream, &cgroup)?) {
        Ok(child) => child,
        Err(detail) => {
            stop(&mut recorder);
            return Err(detail);
        }
    };
    let judged = Command::new(sibling("bijective-control-engine")?)
        .args(["--mode", "paper", "--config"])
        .arg(run.setup.join("config.json"))
        .arg("--store")
        .arg(&run.store)
        .arg("--readiness")
        .arg(run.store.join("run").join("paper").join("control.json"))
        .arg("--drill-seconds")
        .arg(run.seconds.to_string())
        .status()
        .map_err(|e| format!("the control engine could not be started: {e}"));
    stop(&mut session);
    stop(&mut recorder);
    Ok(if judged?.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// The venue and instruments of a drafted profile.
fn read_market(profile: &Path) -> Result<Market, String> {
    let text = std::fs::read_to_string(profile).map_err(|e| {
        format!(
            "{} could not be read ({e}); write it with bijective-profile --out",
            profile.display()
        )
    })?;
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a profile: {e}", profile.display()))?;
    let venue = parsed.get("venue").and_then(serde_json::Value::as_str).map(str::to_string);
    let symbols: Vec<String> = parsed
        .get("instruments")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter().filter_map(|r| r.get("symbol")?.as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default();
    match venue {
        Some(venue) if !symbols.is_empty() => Ok(Market { venue, symbols }),
        _ => Err(format!("{} names no venue or no instrument", profile.display())),
    }
}

/// The recorder for the venue's public market, writing the stream the session and the node read.
fn record(run: &Run, market: &Market) -> Result<Command, String> {
    let mut command = Command::new(sibling("bijective-data-engine")?);
    command
        .arg("--store")
        .arg(&run.store)
        .arg("--readiness")
        .arg(run.store.join("run").join("paper").join("data.json"))
        .args(["--venue", &market.venue, "--profile"])
        .arg(run.setup.join("profile.json"))
        .args(["--endpoint", &run.endpoint, "--instruments", &market.symbols.join(",")])
        .args(["--drill-seconds", &(run.seconds + RECORDER_MARGIN_SECONDS).to_string()]);
    Ok(command)
}

/// The paper session: proposals go on the paper stream, and fills come back to the strategy.
fn session(run: &Run, stream: &str, cgroup: &std::ffi::OsString) -> Result<Command, String> {
    let mut command = Command::new(sibling("bijective-strategy-engine")?);
    command
        .arg("--readiness")
        .arg(run.store.join("run").join("paper").join("strategy.json"))
        .arg("--cgroup-root")
        .arg(cgroup)
        .arg("--store")
        .arg(&run.store)
        .arg("--strategy")
        .arg(&run.strategy)
        .args([
            "--market",
            stream,
            "--stage",
            "paper",
            "--drill-seconds",
            &run.seconds.to_string(),
        ]);
    if run.strategy.extension().is_some_and(|e| e == "py") {
        let library = library_for_python()
            .ok_or("this build carries no Python library for a strategy to import")?;
        command.arg("--python-path").arg(library);
    }
    Ok(command)
}

/// Waits until the recorder has written the first book of the market.
fn first_book(store: &Path, stream: &str) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < FIRST_BOOK_WAIT {
        if matches!(bijective_event_store::store::end_of(store, stream), Ok(Some(_))) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "no book arrived on {stream} within {} s; check the stream endpoint",
        FIRST_BOOK_WAIT.as_secs()
    ))
}

fn sibling(name: &str) -> Result<PathBuf, String> {
    beside_me(name).ok_or_else(|| format!("{name} is not beside this program"))
}

fn spawn(mut command: Command) -> Result<Child, String> {
    command.spawn().map_err(|e| format!("{:?} could not be started: {e}", command.get_program()))
}

/// Each child ends on its own drill bound. One still running when the node is done is stopped.
fn stop(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    let _ = child.wait();
}
