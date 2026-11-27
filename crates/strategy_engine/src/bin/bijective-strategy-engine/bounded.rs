// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A bounded run: a recorded window sent to the strategy, its output recorded and admitted.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use bijective_common::readiness::{Health, Surface};
use bijective_event_store::store::StreamWriter;
use bijective_sandbox::live::{Live, QUEUED_BY_DEFAULT};
use bijective_sandbox::{Completed, Exit, Limits};
use bijective_strategy_engine::ledger::{RUNS, Recorded, attribute, record};
use bijective_strategy_engine::protocol::{Batch, Inbound, Sent, is_turn, to_line};
use bijective_strategy_engine::runs::ledger::Admitted;
use bijective_strategy_engine::runs::{Exchange, RESEARCH_PROPOSALS, Research, RunSpec};
use bijective_strategy_engine::{Admitting, admit_result, supervise};

use crate::PROGRAM;
use crate::markets::{self, Feed, Market};
use crate::run::{Sandbox, limits, now_ns, sandbox};

/// Exchange and environment: with the markets, all part of the run's identity.
pub(crate) struct Bounded {
    pub(crate) exchange: Exchange,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) called: Option<String>,
}

/// Lines a stepwise run wrote, each with what it had been sent when it wrote them.
type Said = Vec<(Sent, String)>;

/// One strategy's run, prepared and then executed: what its settlement needs.
struct Prepared {
    program: std::path::PathBuf,
    spec: RunSpec<Research>,
    sandbox: Sandbox,
}

struct Executed {
    started_ns: i128,
    ended_ns: i128,
    outcome: Result<(Completed, Said), String>,
}

/// Runs every strategy over one window and returns one summary line per strategy, in the order
/// given. The window is one market, or several merged (`markets`), read and decoded once. Every
/// run is sent the same bytes, so the runs differ only in the program. Each run keeps its own
/// sandbox, cgroup, ledger entry and attribution. At most one run per core executes at a time. A
/// run that fails is reported in its line and does not stop the others. Precondition: no program
/// is given twice, and `named` holds each market with the position its window ends at.
pub(crate) fn drive(
    surface: &Surface,
    cgroup_root: &Path,
    store: &Path,
    programs: &[std::path::PathBuf],
    named: &[(String, u64)],
    run: Bounded,
) -> Result<String, String> {
    let Bounded { exchange, environment, called } = run;
    surface.report_or_warn(Health::Replaying, "reading the run's window", now_ns());

    let markets = markets::read(store, named)?;
    let order = markets::merged(&markets);
    let bytes = markets::render(&markets, &order)?;
    let prepared = programs
        .iter()
        .map(|program| {
            let name = called.clone().filter(|_| programs.len() == 1).or_else(|| stem(program));
            let spec = declare(program, &environment, named, exchange, name)?;
            let sandbox = sandbox(
                cgroup_root,
                store,
                program,
                environment.clone(),
                "strategy",
                spec.identity(),
                "run",
            )?;
            Ok(Prepared { program: program.clone(), spec, sandbox })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let window = Window { store, markets: &markets, order: &order, bytes: &bytes, exchange };
    let executed = execute_all(&prepared, &window);

    let mut lines = Vec::new();
    for (run, done) in prepared.iter().zip(executed) {
        let line = match done.outcome {
            Ok((completed, said)) => {
                let times = (done.started_ns, done.ended_ns);
                settle(&window, run, completed, &said, times).unwrap_or_else(|e| e)
            }
            Err(e) => e,
        };
        lines.push(if programs.len() == 1 {
            line
        } else {
            format!("{}: {line}", run.program.display())
        });
    }
    let through: Vec<String> = markets.iter().map(|m| m.to_position.to_string()).collect();
    let summary = format!(
        "{} event(s) sent to position {} {}",
        order.len(),
        through.join(" and "),
        exchange.name()
    );
    Ok(format!("{summary}; {}", lines.join("; ")))
}

/// The name a strategy in a set is known by, so moving the set keeps each history.
fn stem(program: &Path) -> Option<String> {
    program.file_stem().map(|s| s.to_string_lossy().to_string())
}

/// What every run of a set reads and is sent.
#[derive(Clone, Copy)]
struct Window<'a> {
    store: &'a Path,
    markets: &'a [Market],
    /// The merged send order, as (market, index into its updates).
    order: &'a [(usize, usize)],
    /// The whole window rendered, for a whole exchange.
    bytes: &'a [u8],
    exchange: Exchange,
}

impl Window<'_> {
    /// Everything of each market, as a whole exchange sends it.
    fn whole(&self) -> Sent {
        markets::sent(&self.feeds(), |m| Some(Batch::of(0, self.markets[m].to_position)))
    }

    fn feeds(&self) -> Vec<&Feed> {
        self.markets.iter().map(|m| &m.feed).collect()
    }
}

/// Each run in its own thread, in waves of at most one per core.
fn execute_all(prepared: &[Prepared], window: &Window<'_>) -> Vec<Executed> {
    let wave = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut executed = Vec::with_capacity(prepared.len());
    for chunk in prepared.chunks(wave) {
        std::thread::scope(|scope| {
            let handles: Vec<_> =
                chunk.iter().map(|run| scope.spawn(move || execute(run, window))).collect();
            for handle in handles {
                executed.push(handle.join().unwrap_or_else(|_| Executed {
                    started_ns: now_ns(),
                    ended_ns: now_ns(),
                    outcome: Err("the run's thread ended abnormally".to_string()),
                }));
            }
        });
    }
    executed
}

fn execute(run: &Prepared, window: &Window<'_>) -> Executed {
    let Sandbox { cgroup, layout } = &run.sandbox;
    let started_ns = now_ns();
    let outcome = match window.exchange {
        Exchange::Whole => bijective_sandbox::run(cgroup, layout, limits(), window.bytes)
            .map(|completed| (completed, Vec::new()))
            .map_err(|e| format!("the run could not be started: {e}")),
        Exchange::Stepwise => step_through(cgroup, layout, limits(), window),
    };
    Executed { started_ns, ended_ns: now_ns(), outcome }
}

/// Records the run, then admits what it emitted. A run cut short is recorded and admits nothing.
fn settle(
    window: &Window<'_>,
    run: &Prepared,
    completed: Completed,
    said: &Said,
    (started_ns, ended_ns): (i128, i128),
) -> Result<String, String> {
    // A run cut short is not a result: `supervise` withholds its output from the proposal stream.
    let supervised = supervise(&run.spec, &completed, started_ns, ended_ns);
    // Recorded unconditionally, so a restarted node can tell a cut-short run from one never started.
    let mut ledger = StreamWriter::open(window.store, RUNS)
        .map_err(|e| format!("the run ledger could not be opened: {e:?}"))?;
    let recorded = record(&supervised.entry, &mut ledger).map_err(|e| format!("{e}"))?;
    let Some(output) = supervised.output else {
        return Err(format!(
            "the run ended {:?} and what it emitted is not a result; recorded at {}",
            supervised.entry.exit,
            recorded.at()
        ));
    };

    let mut proposals = StreamWriter::open(window.store, RESEARCH_PROPOSALS)
        .map_err(|e| format!("the proposal stream could not be opened: {e:?}"))?;
    let venues =
        admit(window, &output, said, (&recorded, run.spec.strategy_name()), &mut proposals)?;
    let (admitted, refused) = (venues.0.len(), venues.1);
    for line in &refused {
        eprintln!(
            "{PROGRAM}: {}: line {} refused: {}",
            run.program.display(),
            line.line,
            line.because
        );
    }
    attribute_admitted(&mut ledger, &proposals, run.spec.identity(), window, &venues.0)?;
    Ok(format!(
        "recorded at {}, {admitted} proposal(s) admitted, {} refused",
        recorded.at(),
        refused.len()
    ))
}

fn declare(
    program: &Path,
    environment: &BTreeMap<String, String>,
    named: &[(String, u64)],
    exchange: Exchange,
    called: Option<String>,
) -> Result<RunSpec<Research>, String> {
    let to_position = named.first().map(|(_, to)| *to);
    let spec = RunSpec::<Research>::declare(
        program.display().to_string(),
        Vec::new(),
        environment.clone(),
        to_position,
        exchange,
    )
    .map_err(|e| format!("the run could not be declared: {e:?}"))?;
    // One market keeps the identity it always had. Several are part of what the run is.
    let spec = if named.len() > 1 {
        spec.reading(named.iter().map(|(stream, to)| (stream.clone(), Some(*to))).collect())
    } else {
        spec
    };
    Ok(match called {
        Some(name) => spec.called(name),
        None => spec,
    })
}

/// Records which proposal positions this run's admissions landed on, so a measurement prices
/// them against the market the run was sent. `venues` holds the venue each admitted proposal
/// names, in append order. Each stretch of one venue is attributed to that venue's market, so a
/// run of several markets is measured on the book each proposal was for.
fn attribute_admitted(
    ledger: &mut StreamWriter,
    proposals: &StreamWriter,
    specification: bijective_event_store::digest::Digest,
    window: &Window<'_>,
    venues: &[Option<String>],
) -> Result<(), String> {
    let mut from = proposals.next_sequence() - venues.len() as u64;
    for (index, count) in markets::stretches(venues, &window.feeds()) {
        let market = &window.markets[index];
        let landed = Admitted {
            specification,
            from,
            through: from + count - 1,
            count,
            market: market.feed.stream_name.clone(),
            market_through: market.to_position,
            proposals: RESEARCH_PROPOSALS.to_string(),
        };
        attribute(&landed, ledger).map_err(|e| format!("{e}"))?;
        from += count;
    }
    Ok(())
}

/// Admits a run's proposals. Whole: the window is one batch per market. Stepwise: each line is
/// admitted against what had been sent when it was written.
fn admit(
    window: &Window<'_>,
    output: &[u8],
    said: &Said,
    (recorded, strategy): (&Recorded, Option<&str>),
    proposals: &mut StreamWriter,
) -> Result<(Vec<Option<String>>, Vec<bijective_strategy_engine::Refused>), String> {
    match window.exchange {
        Exchange::Whole => {
            let whole = window.whole();
            let admitting = Admitting { sent: &whole, placeable: false, strategy };
            admit_result(output, recorded, &admitting, proposals).map_err(|e| format!("{e}"))
        }
        Exchange::Stepwise => {
            let mut admitted = Vec::new();
            let mut refused = Vec::new();
            for (sent, line) in said {
                let admitting = Admitting { sent, placeable: true, strategy };
                let (a, r) = admit_result(line.as_bytes(), recorded, &admitting, proposals)
                    .map_err(|e| format!("{e}"))?;
                admitted.extend(a);
                refused.extend(r);
            }
            Ok((admitted, refused))
        }
    }
}

/// Sends a window one event at a time and waits for the strategy's turn between events. A
/// strategy answering an event has seen events up to it only: on one market, the event itself.
/// On several, the latest event of each. One that never answers reaches the wall bound and the
/// run is reported as a timeout.
fn step_through(
    cgroup: &bijective_sandbox::cgroup::RunCgroup,
    layout: &bijective_sandbox::RunLayout,
    bound: Limits,
    window: &Window<'_>,
) -> Result<(Completed, Said), String> {
    let markets = window.markets;
    let started = Instant::now();
    // At most one event is unread at any time, so the default queue bound is sufficient.
    let mut strategy = Live::start(cgroup, layout, QUEUED_BY_DEFAULT)
        .map_err(|e| format!("the run could not be started: {e}"))?;
    let mut said: Said = Vec::new();
    let mut cut_short = false;
    let feeds = window.feeds();
    let opened = markets::openings(&feeds);
    if !opened.is_empty() {
        strategy.send(&opened).map_err(|e| format!("the strategy was not told its venue: {e}"))?;
    }

    let mut latest: Vec<Option<u64>> = vec![None; markets.len()];
    for &(m, i) in window.order {
        let line = markets::line(markets, (m, i))?;
        if strategy.send(line.as_bytes()).is_err() {
            // The strategy stopped reading. What it already wrote still stands.
            break;
        }
        latest[m] = Some(markets[m].updates[i].0);
        let answering = markets::sent(&feeds, |k| latest[k].map(|p| Batch::of(p, p)));
        if !await_turn(&mut strategy, started, bound, &answering, &mut said, true)? {
            cut_short = true;
            break;
        }
    }
    if !cut_short {
        let end = to_line(&Inbound::EndOfWindow).map_err(|e| format!("{e}"))?;
        let _ = strategy.send(end.as_bytes());
        // Output after the window answers the last event of each market: nothing else was sent.
        let last = markets::sent(&feeds, |k| {
            let to = markets[k].to_position;
            Some(Batch::of(to, to))
        });
        cut_short = !await_turn(&mut strategy, started, bound, &last, &mut said, false)?;
    }

    let ended = strategy.stop(cgroup).map_err(|e| format!("the run could not be stopped: {e}"))?;
    let mut stdout = String::new();
    for (_, line) in &said {
        stdout.push_str(line);
        stdout.push('\n');
    }
    Ok((
        Completed {
            // A run the supervisor cut short is a timeout whatever the process did afterwards.
            exit: if cut_short { Exit::Timeout } else { ended.exit },
            stdout: stdout.into_bytes(),
            stderr: ended.stderr,
            elapsed: ended.elapsed,
        },
        said,
    ))
}

/// Collects output until the strategy signals its turn (when `stop_on_turn`) or closes its
/// output. Returns false when the wall bound was reached first.
fn await_turn(
    strategy: &mut Live,
    started: Instant,
    bound: Limits,
    sent: &Sent,
    said: &mut Said,
    stop_on_turn: bool,
) -> Result<bool, String> {
    loop {
        let spent = started.elapsed();
        if spent >= bound.wall {
            return Ok(false);
        }
        // Waited for, not polled: an answer is heard at once, and a run that sends nothing still reaches the wall-clock bound.
        let heard = strategy
            .wait(bound.wall - spent)
            .map_err(|e| format!("the strategy's output could not be read: {e}"))?;
        let mut answered = false;
        for line in heard.lines {
            if is_turn(&line) {
                answered = true;
            } else {
                said.push((sent.clone(), line));
            }
        }
        if (stop_on_turn && answered) || heard.closed {
            return Ok(true);
        }
    }
}
