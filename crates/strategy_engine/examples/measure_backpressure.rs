// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Measures what a live session holds for a strategy that stopped reading, and how fast a reading one drains.
//! The arrival rate is an argument: no venue recording lives on this node to derive it from.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bijective_sandbox::live::{Live, Unsent};
use bijective_sandbox::{Limits, RunLayout};
use bijective_strategy_engine::protocol::{book_line, to_line};

fn main() {
    let Some(root) = std::env::var_os("BIJECTIVE_CGROUP_ROOT") else {
        eprintln!(
            "measure-backpressure: set BIJECTIVE_CGROUP_ROOT to a delegated cgroup v2 subtree;"
        );
        eprintln!("                      an unbounded run is not the run a Client gets");
        std::process::exit(1);
    };
    let root = std::path::PathBuf::from(root);

    // `--strategy` times an operator's own program, the half of the arithmetic that is theirs.
    let mut taking: Option<std::path::PathBuf> = None;
    let mut rates: Vec<u64> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--strategy" {
            taking = args.next().map(std::path::PathBuf::from);
        } else if let Ok(rate) = arg.parse() {
            rates.push(rate);
        }
    }
    if rates.is_empty() {
        rates = vec![10, 100, 1_000];
    }

    let work = std::env::temp_dir().join(format!("bijective-backpressure-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("scratch");

    let event = rendered_event();
    println!("EVENT  {} bytes as the supervisor renders it", event.len());

    let absorbed = match absorb(&root, &work, &event) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("measure-backpressure: {e}");
            std::process::exit(1);
        }
    };
    println!("HOLDS  {absorbed} event(s) for a strategy that takes none");

    let drain = match drain(&root, &work, &event, taking.as_deref()) {
        Ok(rate) => rate,
        Err(e) => {
            eprintln!("measure-backpressure: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "DRAINS {drain} event(s)/s to {}",
        taking
            .as_deref()
            .map_or("a compiled strategy that is reading".to_string(), |p| p.display().to_string())
    );
    println!();
    println!("{:>12}  {:>12}  what it means", "arrivals/s", "slack");
    println!("------------------------------------------------------------------");
    for rate in rates {
        // Rounded down: head-room reported generously is head-room an operator does not have.
        let slack_ms = absorbed.saturating_mul(1000) / rate.max(1);
        let verdict = if rate >= drain {
            "arrivals outrun the strategy: a countdown, not slack"
        } else {
            "this long stalled before the session ends"
        };
        println!("{rate:>12}  {slack_ms:>9} ms  {verdict}");
    }

    let _ = std::fs::remove_dir_all(&work);
}

fn rendered_event() -> Vec<u8> {
    let update = bijective_serialization::signal::market::BookUpdate {
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        seq: bijective_model::Sequence::Numbered(1),
        venue_ts_ms: 1_788_000_000_000,
        received_at_ns: None,
        bid_price: 2_654_375,
        bid_quantity: 1_500,
        ask_price: 2_654_500,
        ask_quantity: 900,
        bid_depth: 5,
        ask_depth: 5,
    };
    to_line(&book_line(1, &update, None)).expect("render").into_bytes()
}

fn limits() -> Limits {
    Limits { memory_bytes: 256 * 1024 * 1024, processes: 64, wall: Duration::from_secs(60) }
}

fn layout(work: &std::path::Path, program: &std::path::Path, args: Vec<String>) -> RunLayout {
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    RunLayout { inputs: work.to_path_buf(), program: program.to_path_buf(), args, environment }
}

fn absorb(root: &std::path::Path, work: &std::path::Path, event: &[u8]) -> Result<u64, String> {
    let script = work.join("takes-none.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 120\n").map_err(|e| format!("{e}"))?;

    let cgroup =
        bijective_sandbox::cgroup::RunCgroup::create(root, "backpressure-absorb", limits())
            .map_err(|e| format!("the run could not be bounded: {e:?}"))?;
    let mut live = Live::start(
        &cgroup,
        &layout(work, std::path::Path::new("/bin/sh"), vec![script.display().to_string()]),
        bijective_sandbox::live::QUEUED_BY_DEFAULT,
    )
    .map_err(|e| format!("the strategy could not be started: {e}"))?;

    let mut held = 0u64;
    let started = Instant::now();
    loop {
        match live.send(event) {
            Ok(()) => held += 1,
            Err(Unsent::Behind) => break,
            Err(Unsent::Gone) => {
                return Err("the strategy's input closed before the queue filled".to_string());
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            return Err(format!(
                "still accepting after {held} events and 30s, which is not a bound"
            ));
        }
    }

    let _ = live.stop(&cgroup);
    Ok(held)
}

fn drain(
    root: &std::path::Path,
    work: &std::path::Path,
    event: &[u8],
    taking: Option<&std::path::Path>,
) -> Result<u64, String> {
    let taker = match taking {
        Some(given) => given.to_path_buf(),
        None => std::env::current_exe()
            .map_err(|e| format!("{e}"))?
            .parent()
            .ok_or("no examples directory")?
            .join("turn_taker"),
    };
    if !taker.exists() {
        return Err(format!("{} is missing; build the examples first", taker.display()));
    }

    let cgroup = bijective_sandbox::cgroup::RunCgroup::create(root, "backpressure-drain", limits())
        .map_err(|e| format!("the run could not be bounded: {e:?}"))?;
    let mut live = Live::start(
        &cgroup,
        &layout(work, &taker, Vec::new()),
        bijective_sandbox::live::QUEUED_BY_DEFAULT,
    )
    .map_err(|e| format!("the strategy could not be started: {e}"))?;

    // Enough to amortise process start-up, few enough that a stall fails instead of waiting.
    const EVENTS: u64 = 20_000;
    let started = Instant::now();
    let mut sent = 0u64;
    let mut answered = 0u64;
    while answered < EVENTS {
        while sent < EVENTS {
            match live.send(event) {
                Ok(()) => sent += 1,
                Err(Unsent::Behind) => break,
                Err(Unsent::Gone) => return Err("the strategy stopped reading".to_string()),
            }
        }
        let heard = live.wait(Duration::from_secs(5)).map_err(|e| format!("{e}"))?;
        if heard.lines.is_empty() && heard.closed {
            return Err(format!("the strategy finished after {answered} of {EVENTS}"));
        }
        answered += heard.lines.len() as u64;
        if started.elapsed() > Duration::from_secs(45) {
            break;
        }
    }

    let elapsed = started.elapsed();
    let _ = live.stop(&cgroup);

    let millis = elapsed.as_millis().max(1) as u64;
    Ok(answered * 1000 / millis)
}
