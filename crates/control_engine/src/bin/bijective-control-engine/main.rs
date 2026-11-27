// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The control engine program. Replays limit state, verifies it, and starts with trading halted.

mod attribution;
mod backtest;
mod clock;
mod daily_loss;
mod margin;
mod market;
mod node;
mod paper;
mod reconciliation;
mod startup;
mod stop;
mod unattended;
mod venues;
mod withdraw;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use bijective_common::readiness::{Health, Surface};
use bijective_control_engine::{Accepted, Configuration, RunMode, RunPlan, parse_operators};
use bijective_identity::OperatorRegistry;
use bijective_serialization::signal::node::{BACKTEST, LIVE, NodeStreams, PAPER};

use crate::clock::now_ns;
use crate::node::{Launch, Node};

const PROGRAM: &str = "control-engine";

/// Each mode keeps its record apart (`bijective_serialization::signal::node`), so a paper run or a
/// backtest on a live node's store writes nothing the live node reads.
fn streams(plan: RunPlan) -> NodeStreams {
    match plan.mode() {
        RunMode::Backtest => BACKTEST,
        RunMode::Paper => PAPER,
        RunMode::Live => LIVE,
    }
}

/// A live or paper pass per heartbeat. A backtest passes at most this far apart in recorded time.
pub(crate) const HEARTBEAT: Duration = Duration::from_secs(5);

/// Why the program stops, and the exit code that reports it.
pub(crate) struct Fatal {
    pub(crate) detail: String,
    pub(crate) code: u8,
}

impl Fatal {
    /// A refused invocation or configuration: exit code 2.
    fn usage(detail: impl Into<String>) -> Self {
        Self { detail: detail.into(), code: 2 }
    }

    /// A failure after start-up began: exit code 1.
    pub(crate) fn failure(detail: impl Into<String>) -> Self {
        Self { detail: detail.into(), code: 1 }
    }
}

/// Command-line arguments. Every flag takes one value.
struct Args {
    store_root: Option<PathBuf>,
    readiness: Option<PathBuf>,
    configuration: Option<PathBuf>,
    keystore: Option<PathBuf>,
    credential: Option<String>,
    operators: Option<PathBuf>,
    drill_seconds: Option<u64>,
    market_store: Option<PathBuf>,
    market: Option<String>,
    plan: RunPlan,
}

fn parse_args() -> Result<Args, Fatal> {
    let (mut store_root, mut readiness, mut configuration) = (None, None, None);
    let (mut keystore, mut credential, mut drill_seconds) = (None, None, None);
    let mut operators = None;
    let (mut market_store, mut market) = (None, None);
    let (mut replay_from, mut mode) = (None, None);
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let value = argv.next();
        match (flag.as_str(), value) {
            ("--store", Some(v)) => store_root = Some(PathBuf::from(v)),
            ("--readiness", Some(v)) => readiness = Some(PathBuf::from(v)),
            ("--config", Some(v)) => configuration = Some(PathBuf::from(v)),
            ("--keystore", Some(v)) => keystore = Some(PathBuf::from(v)),
            ("--credential", Some(v)) => credential = Some(v),
            ("--operators", Some(v)) => operators = Some(PathBuf::from(v)),
            ("--drill-seconds", Some(v)) => drill_seconds = v.parse().ok(),
            ("--mode", Some(v)) => mode = Some(v),
            ("--market-store", Some(v)) => market_store = Some(PathBuf::from(v)),
            ("--market", Some(v)) => market = Some(v),
            // Refused rather than ignored: an ignored start would make the run read as live.
            // Whether the start fits the mode is decided by `RunPlan`, in one place.
            ("--replay-from", Some(v)) => {
                replay_from = Some(v.parse::<i128>().map_err(|_| {
                    Fatal::usage(format!("--replay-from wants a nanosecond instant, not {v}"))
                })?);
            }
            (other, _) => return Err(Fatal::usage(format!("unusable argument {other}"))),
        }
    }
    let plan =
        RunPlan::select(mode.as_deref(), replay_from).map_err(|e| Fatal::usage(e.to_string()))?;
    Ok(Args {
        store_root,
        readiness,
        configuration,
        keystore,
        credential,
        operators,
        drill_seconds,
        market_store,
        market,
        plan,
    })
}

/// Read and accepted before the store is opened, so a refusal changes nothing on disk.
fn load_configuration(path: &std::path::Path) -> Result<Accepted, Fatal> {
    std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|raw| {
            Configuration::parse(&raw).and_then(Configuration::accept).map_err(|e| e.to_string())
        })
        .map_err(|detail| Fatal::usage(format!("configuration refused: {detail}")))
}

/// The registry of who may sign commands. Live mode refuses to start without one: the role a
/// command claims is not evidence of anything.
fn load_operators(
    path: Option<&std::path::Path>,
    plan: RunPlan,
) -> Result<Option<OperatorRegistry>, Fatal> {
    let Some(path) = path else {
        return if plan.mode() == RunMode::Live {
            Err(Fatal::usage("live mode needs --operators: commands must be signed"))
        } else {
            Ok(None)
        };
    };
    let raw =
        std::fs::read(path).map_err(|e| Fatal::usage(format!("operators file unreadable: {e}")))?;
    parse_operators(&raw).map(Some).map_err(Fatal::usage)
}

fn exit(fatal: Fatal) -> ExitCode {
    eprintln!("{PROGRAM}: {}", fatal.detail);
    ExitCode::from(fatal.code)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(fatal) => return exit(fatal),
    };
    if let Err(detail) = clock::fix(args.plan) {
        return exit(Fatal::failure(detail));
    }
    let (Some(store_root), Some(readiness), Some(configuration)) =
        (args.store_root, args.readiness, args.configuration)
    else {
        return exit(Fatal::usage("--store, --readiness and --config are required"));
    };
    let accepted = match load_configuration(&configuration) {
        Ok(a) => a,
        Err(fatal) => return exit(fatal),
    };

    let operators = match load_operators(args.operators.as_deref(), args.plan) {
        Ok(o) => o,
        Err(fatal) => return exit(fatal),
    };
    let unattended = operators.is_none() && args.plan.mode() != RunMode::Live;
    let recorded = (args.market_store, args.market);
    let window = match backtest::Window::from_args(args.plan, recorded, args.drill_seconds) {
        Ok(w) => w,
        Err(fatal) => return exit(fatal),
    };
    let inputs_root =
        window.as_ref().map_or_else(|| store_root.clone(), |w| w.market_store.clone());

    let surface = Surface::new(&readiness, PROGRAM);
    if let Err(e) =
        surface.report(Health::Starting, "trading halted; about to replay limit state", now_ns())
    {
        return exit(Fatal::usage(format!("the readiness surface could not be written: {e}")));
    }
    let launch = Launch {
        store_root,
        inputs_root,
        plan: args.plan,
        keystore: args.keystore,
        credential: args.credential,
        operators,
    };
    let result = match Node::start(launch, accepted, &surface) {
        // Before the first pass: the first command a pass carries out may be a resume.
        Ok(mut node) => match start_trading(&mut node, args.plan, unattended).await {
            Ok(()) => match &window {
                Some(window) => backtest::run(node, &surface, window).await,
                None => run(node, &surface, args.drill_seconds, args.plan).await,
            },
            Err(fatal) => Err(fatal),
        },
        Err(fatal) => Err(fatal),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(fatal) => {
            surface.report_or_warn(Health::Failed, &fatal.detail, now_ns());
            exit(fatal)
        }
    }
}

/// One pass per heartbeat until `--drill-seconds` have passed or a stop is requested, which is
/// carried out between passes. A paper run then reconciles once more, so a fill its simulated
/// venues made after the last pass is recorded and not lost with the process, and reports what
/// they came to.
async fn run(
    mut node: Node,
    surface: &Surface,
    drill_seconds: Option<u64>,
    plan: RunPlan,
) -> Result<(), Fatal> {
    let mut stop = match stop::Stop::register() {
        Ok(stop) => Some(stop),
        Err(e) => {
            eprintln!("{PROGRAM}: a stop signal ends the process mid-pass: {e}");
            None
        }
    };
    let mut elapsed = 0u64;
    while !drill_seconds.is_some_and(|d| elapsed >= d) {
        node.pass(surface).await?;
        let requested = match stop.as_mut() {
            Some(stop) => stop.requested_within(HEARTBEAT).await,
            None => {
                tokio::time::sleep(HEARTBEAT).await;
                false
            }
        };
        if requested {
            eprintln!("{PROGRAM}: stop requested; stopping between passes");
            break;
        }
        elapsed += HEARTBEAT.as_secs();
    }
    if plan.mode() == RunMode::Paper {
        node.reconcile_at_close().await?;
        backtest::report_paper(&node);
    }
    Ok(())
}

/// Reconciles before the first pass, which may carry out a resume, and appends that resume when
/// nobody could sign one.
async fn start_trading(node: &mut Node, plan: RunPlan, unattended: bool) -> Result<(), Fatal> {
    node.reconcile_at_start().await?;
    if unattended {
        unattended::resume_itself(node, streams(plan).commands, now_ns())?;
    }
    Ok(())
}
