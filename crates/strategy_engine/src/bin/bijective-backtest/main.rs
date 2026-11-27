// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One command to run a strategy over a market this node has recorded.
//! Finds what can be found, takes the conservative option where there is one, and prints every choice.
//! It will not guess a cgroup root: a run that is not bounded is not the run an operator gets.

mod order_path;
mod starter;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bijective_strategy_engine::launch::{Bounded, beside_me, library_for_python};

pub(crate) const PROGRAM: &str = "bijective-backtest";

/// What an operator wrote down once. A file says what is true of the work. Flags win, being what was just typed.
#[derive(Debug, Default)]
struct Project {
    /// Its identity for the promotion stages, so moving the file keeps its history.
    name: Option<String>,
    store: Option<String>,
    market: Option<String>,
    exchange: Option<String>,
    confidence: Option<u64>,
    /// The node configuration the order path runs under (decision 0015).
    config: Option<String>,
}

pub(crate) const PROJECT_FILE: &str = "bijective.json";

/// Read beside the operator, not beside the binary. A project is a directory they are standing in.
fn read_project() -> Project {
    let Ok(raw) = std::fs::read(PROJECT_FILE) else {
        return Project::default();
    };
    let parsed: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            // Said, not swallowed: a project file that does not parse is one an operator edited and expects to be in force.
            eprintln!(
                "{PROGRAM}: {PROJECT_FILE} could not be read ({e}); its settings are not in use"
            );
            return Project::default();
        }
    };
    let text = |key: &str| parsed.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Project {
        name: text("name"),
        store: text("store"),
        market: text("market"),
        exchange: text("exchange"),
        confidence: parsed.get("confidence").and_then(serde_json::Value::as_u64),
        config: text("config"),
    }
}

/// Why the program stops early, with the exit code that reports it. What to say is said before.
struct Stop(ExitCode);

impl Stop {
    /// A refused invocation: exit code 2.
    fn usage(detail: impl std::fmt::Display) -> Self {
        eprintln!("{PROGRAM}: {detail}");
        Self(ExitCode::from(2))
    }

    /// A failure after the invocation was accepted: exit code 1.
    fn failure(detail: impl std::fmt::Display) -> Self {
        eprintln!("{PROGRAM}: {detail}");
        Self(ExitCode::FAILURE)
    }
}

/// What was typed. Every option but the strategy can also come from the project file.
#[derive(Default)]
struct Args {
    strategy: Option<PathBuf>,
    store: Option<PathBuf>,
    market: Option<String>,
    through: Option<u64>,
    fast: bool,
    scaffold: Option<PathBuf>,
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Args, Stop> {
    let mut parsed = Args::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => parsed.store = args.next().map(PathBuf::from),
            "--market" => parsed.market = args.next(),
            "--through" => match args.next().map(|v| v.parse::<u64>()) {
                Some(Ok(p)) => parsed.through = Some(p),
                _ => return Err(Stop::usage("--through wants a market position")),
            },
            "--fast" => parsed.fast = true,
            "--new" => parsed.scaffold = args.next().map(PathBuf::from),
            "--config" => parsed.config = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                usage();
                return Err(Stop(ExitCode::SUCCESS));
            }
            other if other.starts_with("--") => {
                usage();
                return Err(Stop::usage(format!("unusable argument {other}")));
            }
            other => parsed.strategy = Some(PathBuf::from(other)),
        }
    }
    Ok(parsed)
}

fn main() -> ExitCode {
    let outcome = parse_args().and_then(|args| match args.scaffold.clone() {
        Some(path) => starter::scaffold(&path, args.store.as_deref()),
        None => run(args),
    });
    match outcome {
        Ok(code) | Err(Stop(code)) => code,
    }
}

/// Everything a run is made of, resolved and printed before it starts.
struct Window {
    strategies: Vec<PathBuf>,
    store: PathBuf,
    market: String,
    through: u64,
    exchange: &'static str,
}

fn run(args: Args) -> Result<ExitCode, Stop> {
    let Some(strategy) = args.strategy.as_deref() else {
        usage();
        return Err(Stop(ExitCode::from(2)));
    };
    let strategies = strategies_at(strategy).map_err(Stop::usage)?;
    if strategies.len() > 1 {
        println!("set:      {} strategies, one window, one run each", strategies.len());
    }
    let project = read_project();
    let store = resolve_store(args.store, &project)?;
    let cgroup_root = bounded_by()?;
    let market = resolve_market(args.market, &project, &store)?;
    let through = resolve_through(args.through, &store, &market)?;
    let exchange = exchange_of(args.fast, &project);
    let window = Window { strategies, store, market, through, exchange };

    let Some(supervisor) = beside_me("bijective-strategy-engine") else {
        return Err(Stop::failure("the supervisor is not beside this program"));
    };
    let arguments = supervisor_arguments(&window, &project, cgroup_root)?;
    let rendered: Vec<String> = arguments.iter().map(|a| a.to_string_lossy().to_string()).collect();
    println!("running:  {} {}", supervisor.display(), rendered.join(" "));
    println!();

    let config = order_path::configuration(args.config, project.config.as_deref());
    let before = proposals_before(config.as_deref(), &window)?;
    let status = std::process::Command::new(&supervisor)
        .args(&arguments)
        .status()
        .map_err(|e| Stop::failure(format!("the supervisor could not be started: {e}")))?;
    if !status.success() {
        return Ok(ExitCode::FAILURE);
    }
    if window.strategies.len() > 1 {
        return Ok(ExitCode::SUCCESS);
    }
    println!();
    place(config.as_deref(), &window, before)?;
    println!();
    println!(
        "next:     bijective-promote {} {} {}",
        window.store.display(),
        window.strategies[0].display(),
        project.confidence.map_or("<confidence, e.g. 19600>".to_string(), |c| c.to_string())
    );
    Ok(ExitCode::SUCCESS)
}

/// The subtree the run is bounded under, or the stop that ends this program.
fn bounded_by() -> Result<std::ffi::OsString, Stop> {
    match bijective_strategy_engine::launch::bounded() {
        Ok(Bounded::Under(root)) => Ok(root),
        Ok(Bounded::RanInScope(code)) => Err(Stop(code)),
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            bijective_strategy_engine::launch::say_how_to_bound(PROGRAM);
            Err(Stop(ExitCode::from(2)))
        }
    }
}

fn resolve_store(flag: Option<PathBuf>, project: &Project) -> Result<PathBuf, Stop> {
    let store = flag
        .or_else(|| project.store.as_deref().map(PathBuf::from))
        .or_else(|| std::env::var_os("BIJECTIVE_STORE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("store"));
    if !store.is_dir() {
        return Err(Stop::usage(format!(
            "no store at {} - name one with --store",
            store.display()
        )));
    }
    Ok(store.canonicalize().unwrap_or(store))
}

fn resolve_market(flag: Option<String>, project: &Project, store: &Path) -> Result<String, Stop> {
    if let Some(market) = flag.or_else(|| project.market.clone()) {
        return Ok(market);
    }
    let market = found_market(store).map_err(Stop::usage)?;
    println!("market:   {market} (the only one recorded here)");
    Ok(market)
}

fn resolve_through(flag: Option<u64>, store: &Path, market: &str) -> Result<u64, Stop> {
    if let Some(through) = flag {
        return Ok(through);
    }
    match bijective_event_store::store::end_of(store, market) {
        Ok(Some(end)) => {
            println!("window:   0 to {end}, which is everything recorded");
            Ok(end)
        }
        Ok(None) => Err(Stop::usage(format!("{market} holds nothing to run against"))),
        Err(e) => Err(Stop::usage(format!("{market} could not be read: {e:?}"))),
    }
}

/// Stepwise unless asked otherwise: the two are different runs, and the conservative one can carry a claim.
fn exchange_of(fast: bool, project: &Project) -> &'static str {
    let exchange = match (fast, project.exchange.as_deref()) {
        (true, _) | (false, Some("whole")) => "whole",
        _ => "stepwise",
    };
    println!(
        "exchange: {exchange}{}",
        if fast { " - fast, and its figures cannot be sized" } else { "" }
    );
    exchange
}

/// Built once for the printed line and the process, so what an operator is shown is what ran.
fn supervisor_arguments(
    window: &Window,
    project: &Project,
    cgroup_root: std::ffi::OsString,
) -> Result<Vec<std::ffi::OsString>, Stop> {
    let readiness = window.store.join("run").join("backtest.json");
    if let Some(parent) = readiness.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut arguments: Vec<std::ffi::OsString> = vec![
        "--readiness".into(),
        readiness.into(),
        "--cgroup-root".into(),
        cgroup_root,
        "--store".into(),
        window.store.clone().into(),
        "--market".into(),
        window.market.clone().into(),
        "--to-position".into(),
        window.through.to_string().into(),
        "--exchange".into(),
        window.exchange.into(),
    ];
    for path in &window.strategies {
        arguments.push("--strategy".into());
        arguments.push(path.clone().into());
    }

    // A declared name is what the promotion stages follows. Otherwise moving the file begins a second, unrelated history.
    // A set names each strategy by its file instead.
    if let Some(called) = project.name.as_deref().filter(|_| window.strategies.len() == 1) {
        println!("name:     {called} (its identity, so moving the file keeps its history)");
        arguments.push("--name".into());
        arguments.push(called.into());
    }

    // A run's environment is built from nothing, so PYTHONPATH has to be declared.
    if window.strategies.iter().any(|s| s.extension().is_some_and(|e| e == "py")) {
        let Some(library) = library_for_python() else {
            eprintln!("{PROGRAM}: this build carries no Python library, and a strategy that");
            return Err(Stop::failure(
                "           imports it would fail inside the sandbox, where the error is not shown",
            ));
        };
        println!("library:  {} (declared as PYTHONPATH)", library.display());
        arguments.push("--python-path".into());
        arguments.push(library.into());
    }
    Ok(arguments)
}

/// Where the proposal stream ended before the run, when the order path will run after it.
fn proposals_before(config: Option<&Path>, window: &Window) -> Result<Option<u64>, Stop> {
    match config {
        Some(_) if window.strategies.len() == 1 => {
            order_path::proposals_end(&window.store).map_err(Stop::failure)
        }
        Some(_) => {
            println!("order path: a set is not placed; run one strategy to see its fills");
            Ok(None)
        }
        None => Ok(None),
    }
}

/// The order path over what the run proposed, when a node configuration names the venues.
fn place(config: Option<&Path>, window: &Window, before: Option<u64>) -> Result<(), Stop> {
    let Some(config) = config else {
        println!(
            "order path: not run; name a node configuration with --config, or \"config\" in \
             {PROJECT_FILE}, to see fills, positions and P&L on simulated venues"
        );
        return Ok(());
    };
    let Some(control_engine) = beside_me("bijective-control-engine") else {
        return Err(Stop::failure("the control engine is not beside this program"));
    };
    let ran = order_path::Ran { store: &window.store, market: &window.market, config, before };
    match order_path::run(&ran, &control_engine) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Stop(ExitCode::FAILURE)),
        Err(e) => Err(Stop::failure(e)),
    }
}

fn usage() {
    eprintln!(
        "usage: {PROGRAM} <strategy | directory> [--market <stream>] [--through <position>] [--fast] [--store <dir>] [--config <file>]"
    );
    eprintln!("       --config places the proposals on simulated venues through the order path");
    eprintln!("       a directory runs every .py strategy in it over the same window");
    eprintln!("       {PROGRAM} --new <path>    write a strategy that already runs");
}

/// The market stream in this store, where there is exactly one.
/// An operator with several must name one: picking for them would run a verdict about a market they did not mean.
pub(crate) fn found_market(store: &Path) -> Result<String, String> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(store)
        .map_err(|e| format!("{} could not be read: {e}", store.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("market-") && entry.path().is_dir() {
            found.push(name);
        }
    }
    found.sort();

    match found.len() {
        0 => Err(format!(
            "no market is recorded in {} - record one, or name it with --market",
            store.display()
        )),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "several markets are recorded here; name one with --market: {}",
            found.join(", ")
        )),
    }
}

/// The strategy, or every `.py` strategy in a directory in name order, made absolute: a run
/// starts in its own inputs directory, so a relative path resolves elsewhere and reads as a
/// broken strategy (exit 127).
fn strategies_at(path: &Path) -> Result<Vec<PathBuf>, String> {
    let resolved =
        path.canonicalize().map_err(|e| format!("{} is not there: {e}", path.display()))?;
    if !resolved.is_dir() {
        return Ok(vec![resolved]);
    }
    let listing = std::fs::read_dir(&resolved)
        .map_err(|e| format!("{} could not be read: {e}", path.display()))?;
    let mut found: Vec<PathBuf> = listing
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "py"))
        .collect();
    found.sort();
    if found.is_empty() {
        return Err(format!("{} holds no .py strategy", path.display()));
    }
    Ok(found)
}
