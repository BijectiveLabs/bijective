// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A live session: resumes at the present of each market stream it reads, sends events as they
//! are recorded and admits proposals as the strategy writes them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use bijective_common::readiness::{Health, Surface};
use bijective_event_store::cursor::{Cursor, read_after};
use bijective_event_store::digest::{Digest, Sha256};
use bijective_event_store::projection::Recovered;
use bijective_event_store::store::StreamWriter;
use bijective_sandbox::cgroup::RunCgroup;
use bijective_sandbox::live::Live;
use bijective_serialization::signal::feedback::{decode as decode_feedback, stream_for};
use bijective_strategy_engine::ledger::{RUNS, attribute, record};
use bijective_strategy_engine::protocol::{Inbound, Missed, order_line, to_line};
use bijective_strategy_engine::runs::ledger::Admitted;
use bijective_strategy_engine::runs::{Exchange, RunLedgerEntry, RunSpec, Stage};
use bijective_strategy_engine::session::{MARK_EVERY, SentThrough};
use bijective_strategy_engine::{Admitting, Behind, Fed, admit_lines, read_fed_after};

use crate::PROGRAM;
use crate::markets::{self, Along, Source, sources, unanswered};
use crate::run::{Sandbox, mark, now_ns, sandbox};

const SESSIONS: &str = "strategy-sessions";

/// What one poll read of one market stream, in stream order.
type Gained<'a> = &'a [(u64, Fed)];

/// Interval between reads of the market streams. Short, because a live strategy cannot act on
/// events recorded during the interval.
const POLL: Duration = Duration::from_millis(20);

/// Stage, optional duration, queue tolerance, environment and declared name of a live session.
pub(crate) struct Session {
    pub(crate) stage: Stage,
    pub(crate) seconds: Option<u64>,
    pub(crate) tolerated: Behind,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) called: Option<String>,
}

/// The streams a session writes to.
struct Streams {
    proposals: StreamWriter,
    marks: StreamWriter,
    ledger: StreamWriter,
}

/// Positions and counts carried across polls, one `Along` per market stream in the order named.
struct Progress {
    along: Vec<Along>,
    unmarked: u64,
    admitted: usize,
    refused: usize,
}

/// One strategy of a session: its sandbox, its process and how far it has been sent.
struct Seat {
    program: PathBuf,
    spec: RunSpec<bijective_strategy_engine::runs::Live>,
    identity: Digest,
    cgroup: RunCgroup,
    strategy: Live,
    /// What it missed on each market since its last session, in the order the markets were named.
    gap_at_start: Vec<Missed>,
    progress: Progress,
    // Digested as it goes: live output is never held whole.
    emitted: Sha256,
    /// Why this strategy stopped being fed, once it has.
    ended: Option<String>,
    /// Proposal positions this strategy's proposals landed on in this session: what it is told about.
    proposed: BTreeSet<u64>,
}

/// Where a live session reads what became of its proposals, and how far it has read.
struct Listening {
    feedback_stream: String,
    cursor: Option<Cursor>,
}

impl Listening {
    /// From the end of the stream: a session is told about its own proposals, which are all new.
    /// Each stage hears the node that reads its proposals, and no other.
    fn from_now(store: &Path, stage: Stage) -> Result<Self, String> {
        let node = match stage {
            Stage::Live => bijective_serialization::signal::node::LIVE,
            Stage::Paper => bijective_serialization::signal::node::PAPER,
        };
        let feedback_stream = stream_for(node.control);
        let (_, cursor) = read_after(store, &feedback_stream, None)
            .map_err(|e| format!("the order feedback could not be read: {e:?}"))?;
        Ok(Self { feedback_stream, cursor })
    }
}

/// Runs every strategy given live over every market named, read once per poll, until the
/// session's time is up or every strategy has closed its output. Returns one summary line per
/// strategy, in the order given. All of them write the stage's one proposal stream through this
/// process, which is its only writer, and each is attributed by its run identity. A strategy that
/// fails is ended and recorded on its own. The others continue. Precondition: `streams` is not
/// empty, and several name one venue each.
pub(crate) fn live(
    surface: &Surface,
    cgroup_root: &Path,
    store: &Path,
    programs: &[PathBuf],
    streams: &[String],
    session: Session,
) -> Result<String, String> {
    let Session { stage, seconds, tolerated, environment, called } = session;
    let sources = sources(store, streams)?;
    let seating =
        Seating { cgroup_root, store, environment: &environment, tolerated, sources: &sources };
    let mut seats = Vec::with_capacity(programs.len());
    for program in programs {
        let name = called.clone().filter(|_| programs.len() == 1).or_else(|| stem(program));
        let opened = take_seat(&seating, program, name)?;
        seats.push(opened);
    }
    surface.report_or_warn(Health::Ready, "strategies are live", now_ns());

    let mut writers = open_streams(store, stage)?;
    let started_ns = now_ns();
    let context = Context { store, sources: &sources, stage, tolerated };
    let mut listening = Some(Listening::from_now(store, stage)?);
    let deadline = seconds.map(|s| std::time::Instant::now() + Duration::from_secs(s));
    let mut cursors: Vec<Option<Cursor>> = vec![None; sources.len()];
    loop {
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break;
        }
        poll(&context, &mut seats, &mut writers, &mut cursors)?;
        if let Some(listening) = listening.as_mut() {
            tell(store, &mut seats, listening)?;
        }
        if seats.iter().all(|s| s.ended.is_some()) {
            break;
        }
        sleep(POLL);
    }

    let mut lines = Vec::with_capacity(seats.len());
    for seat in seats {
        let line = leave(seat, &mut writers, &context, started_ns)?;
        lines.push(line);
    }
    Ok(lines.join("; "))
}

/// The name a strategy in a set is known by, so moving the set keeps each history.
fn stem(program: &Path) -> Option<String> {
    program.file_stem().map(|s| s.to_string_lossy().to_string())
}

/// What every strategy of a session is seated with.
struct Seating<'a> {
    cgroup_root: &'a Path,
    store: &'a Path,
    environment: &'a BTreeMap<String, String>,
    tolerated: Behind,
    sources: &'a [Source],
}

fn take_seat(
    seating: &Seating<'_>,
    program: &Path,
    called: Option<String>,
) -> Result<Seat, String> {
    let Seating { cgroup_root, store, environment, tolerated, sources } = *seating;
    let spec = declare(program, environment, called, sources)?;
    let identity = spec.identity();
    let only = sources.len() == 1;
    let missed = sources
        .iter()
        .map(|s| missed_since_last_session(store, identity, &s.feed.stream_name, only))
        .collect::<Result<Vec<_>, String>>()?;
    let Sandbox { cgroup, layout } =
        sandbox(cgroup_root, store, program, environment.clone(), "live", identity, "session")?;
    let mut strategy = Live::start(&cgroup, &layout, tolerated.events())
        .map_err(|e| format!("the strategy could not be started: {e}"))?;
    for (source, missed) in sources.iter().zip(&missed) {
        tell_where_it_resumes(&mut strategy, source, missed)?;
        if let Some(line) = &source.feed.opening {
            strategy
                .send(line)
                .map_err(|e| format!("the strategy could not be told its venue: {e}"))?;
        }
    }
    Ok(Seat {
        program: program.to_path_buf(),
        spec,
        identity,
        cgroup,
        strategy,
        gap_at_start: missed,
        proposed: BTreeSet::new(),
        progress: Progress {
            along: sources.iter().map(|s| Along::starting_at(s.resume_from)).collect(),
            unmarked: 0,
            admitted: 0,
            refused: 0,
        },
        emitted: Sha256::new(),
        ended: None,
    })
}

/// Marks how far the strategy was sent, stops it and records the session in the run ledger.
fn leave(
    seat: Seat,
    streams: &mut Streams,
    context: &Context<'_>,
    started_ns: i128,
) -> Result<String, String> {
    let Seat {
        program,
        spec,
        identity,
        cgroup,
        strategy,
        gap_at_start,
        progress,
        emitted,
        ended,
        ..
    } = seat;
    // Written however the session ended: a clean stop and a kill leave the next session needing
    // the same position.
    mark_all(&mut streams.marks, identity, context.sources, &progress.along)?;
    let stopped =
        strategy.stop(&cgroup).map_err(|e| format!("the strategy could not be stopped: {e}"))?;
    let entry = ledger_entry(&spec, started_ns, &stopped.exit, emitted);
    let recorded = record(&entry, &mut streams.ledger).map_err(|e| format!("{e}"))?;
    let ended = ended.unwrap_or_else(|| "the session's time was up".to_string());
    let resumed: Vec<String> = progress.along.iter().map(|a| a.resume_from.to_string()).collect();
    let gaps: Vec<String> = gap_at_start.iter().map(gap).collect();
    let through: Vec<String> = progress
        .along
        .iter()
        .map(|a| a.through.map_or("nothing".to_string(), |t| t.to_string()))
        .collect();
    Ok(format!(
        "{}: {ended}; {} session resumed at {} after {}, sent through {}, {} proposal(s) admitted to {}, {} refused, tolerating {}, recorded at {} as {:?}",
        program.display(),
        context.stage.name(),
        resumed.join(" and "),
        gaps.join(" and "),
        through.join(" and "),
        progress.admitted,
        context.stage.proposals(),
        progress.refused,
        context.tolerated,
        recorded.at(),
        entry.exit,
    ))
}

/// Records how far the strategy was sent through each market it was sent anything of.
fn mark_all(
    marks: &mut StreamWriter,
    identity: Digest,
    sources: &[Source],
    along: &[Along],
) -> Result<(), String> {
    for (source, along) in sources.iter().zip(along) {
        if let Some(through) = along.through {
            mark(marks, identity, (&source.feed.stream_name, through), now_ns())?;
        }
    }
    Ok(())
}

/// Fed as the market arrives. Each answer is bounded to the events sent before it. A session of
/// several markets is a different experiment from one of any of them, so the markets are part of
/// its identity. A session of one market keeps the identity it always had.
fn declare(
    program: &Path,
    environment: &BTreeMap<String, String>,
    called: Option<String>,
    sources: &[Source],
) -> Result<RunSpec<bijective_strategy_engine::runs::Live>, String> {
    let spec = RunSpec::<bijective_strategy_engine::runs::Live>::declare(
        program.display().to_string(),
        Vec::new(),
        environment.clone(),
        None,
        Exchange::Stepwise,
    )
    .map_err(|e| format!("the session could not be declared: {e:?}"))?;
    let spec = if sources.len() > 1 {
        spec.reading(sources.iter().map(|s| (s.feed.stream_name.clone(), None)).collect())
    } else {
        spec
    };
    Ok(match called {
        Some(name) => spec.called(name),
        None => spec,
    })
}

/// The first lines a strategy reads, one per market: the position it resumes at and what it
/// missed since the last session.
fn tell_where_it_resumes(
    strategy: &mut Live,
    source: &Source,
    missed: &Missed,
) -> Result<(), String> {
    let opening = to_line(&Inbound::Resumed {
        from_position: source.resume_from.to_string(),
        missed: missed.clone(),
        venue: source.feed.venue.clone(),
        stream: Some(source.feed.stream_name.clone()),
    })
    .map_err(|e| format!("the resume line could not be rendered: {e}"))?;
    strategy
        .send(opening.as_bytes())
        .map_err(|e| format!("the strategy could not be told where it resumes: {e}"))
}

/// Live proposals were judged as they arrived, so the entry records how the session ended and
/// which bytes ran, so two sessions under one name can be told apart.
fn ledger_entry(
    spec: &RunSpec<bijective_strategy_engine::runs::Live>,
    started_ns: i128,
    exit: &bijective_sandbox::Exit,
    emitted: Sha256,
) -> RunLedgerEntry {
    RunLedgerEntry {
        specification: spec.identity(),
        class: spec.class().to_string(),
        started_ns,
        ended_ns: now_ns(),
        exit: bijective_strategy_engine::reason_of(exit),
        emitted: emitted.finish(),
        strategy: Some(spec.strategy()),
        program: Some(spec.program().to_string()),
        content: std::fs::read(spec.program())
            .ok()
            .map(|bytes| bijective_event_store::digest::sha256(&bytes)),
    }
}

fn gap(missed: &Missed) -> String {
    match missed {
        Missed::From { position } => format!("a gap from {position}"),
        Missed::NotRecorded => "a gap nothing records".to_string(),
    }
}

/// What does not change during a session.
struct Context<'a> {
    store: &'a Path,
    sources: &'a [Source],
    stage: Stage,
    tolerated: Behind,
}

/// The position after the last one the previous session of this strategy was sent on `stream`.
/// An incomplete rebuild reads as not recorded, never as no gap.
fn missed_since_last_session(
    store: &Path,
    identity: Digest,
    stream: &str,
    only_stream: bool,
) -> Result<Missed, String> {
    let sent = Recovered::<SentThrough>::rebuild(store, SESSIONS)
        .map_err(|e| format!("the session marks could not be read: {e:?}"))?;
    Ok(match sent.state().and_then(|s| s.of(&identity, stream, only_stream)) {
        Some(through) => Missed::From { position: (through + 1).to_string() },
        None => Missed::NotRecorded,
    })
}

/// Proposals go to the stream the stage names: a paper session never writes the stream the
/// control engine reads.
fn open_streams(store: &Path, stage: Stage) -> Result<Streams, String> {
    Ok(Streams {
        proposals: StreamWriter::open(store, stage.proposals())
            .map_err(|e| format!("the proposal stream could not be opened: {e:?}"))?,
        marks: StreamWriter::open(store, SESSIONS)
            .map_err(|e| format!("the session stream could not be opened: {e:?}"))?,
        ledger: StreamWriter::open(store, RUNS)
            .map_err(|e| format!("the run ledger could not be opened: {e:?}"))?,
    })
}

/// Reads what each market stream gained once, merges it in arrival order and offers it to every
/// strategy still fed, then admits what each wrote, in the order the strategies were given. Each
/// stream is read on from its cursor. Every strategy still fed has been sent all it gained.
fn poll(
    context: &Context<'_>,
    seats: &mut [Seat],
    streams: &mut Streams,
    cursors: &mut [Option<Cursor>],
) -> Result<(), String> {
    if seats.iter().all(|s| s.ended.is_some()) {
        return Ok(());
    }
    let mut gained = Vec::with_capacity(context.sources.len());
    for (s, source) in context.sources.iter().enumerate() {
        let from = seats
            .iter()
            .filter(|seat| seat.ended.is_none())
            .map(|seat| seat.progress.along[s].next)
            .min()
            .unwrap_or(source.resume_from);
        let read = read_fed_after(context.store, &source.feed.stream_name, cursors[s]);
        let (mut fed, cursor) = read.map_err(|e| format!("{e}"))?;
        fed.retain(|(position, _)| *position >= from);
        cursors[s] = cursor.or(cursors[s]);
        gained.push(fed);
    }
    let gained: Vec<Gained<'_>> = gained.iter().map(Vec::as_slice).collect();
    let order = markets::merged_order(&gained);
    for seat in seats.iter_mut().filter(|s| s.ended.is_none()) {
        feed(context, seat, (&gained, &order), streams)?;
        admit_heard(context, seat, streams)?;
    }
    Ok(())
}

/// Sends the events this strategy has not been sent, in the merged order, and marks its progress
/// every `MARK_EVERY`. A strategy that cannot be sent an event is ended, not the session.
fn feed(
    context: &Context<'_>,
    seat: &mut Seat,
    (gained, order): (&[Gained<'_>], &[(usize, usize)]),
    streams: &mut Streams,
) -> Result<(), String> {
    let progress = &mut seat.progress;
    for &(s, i) in order {
        let (position, update) = &gained[s][i];
        let along = &mut progress.along[s];
        if *position < along.next {
            continue;
        }
        let venue = context.sources[s].feed.venue.as_deref();
        let line = update.line(*position, venue).map_err(|e| format!("{e}"))?;
        if let Err(e) = seat.strategy.send(line.as_bytes()) {
            seat.ended = Some(format!(
                "it was not sent position {position}: {e} - tolerating {}",
                context.tolerated
            ));
            return Ok(());
        }
        along.next = position + 1;
        along.through = Some(*position);
        progress.unmarked += 1;
    }
    if progress.unmarked >= MARK_EVERY {
        mark_all(&mut streams.marks, seat.identity, context.sources, &progress.along)?;
        progress.unmarked = 0;
    }
    Ok(())
}

/// Admits what the strategy wrote against the events sent since its last answer.
fn admit_heard(
    context: &Context<'_>,
    seat: &mut Seat,
    streams: &mut Streams,
) -> Result<(), String> {
    let heard = match seat.strategy.hear() {
        Ok(heard) => heard,
        Err(e) => {
            seat.ended = Some(format!("its output could not be read: {e}"));
            return Ok(());
        }
    };
    for line in &heard.lines {
        seat.emitted.update(line.as_bytes());
        seat.emitted.update(b"\n");
    }
    let progress = &mut seat.progress;
    // Only the events since the last answer: a decision on an earlier batch was already answered.
    let sent = unanswered(context.sources, &progress.along);
    let admitting = Admitting { sent: &sent, placeable: true, strategy: seat.spec.strategy_name() };
    let (venues, refused) =
        admit_lines(heard.lines.iter().map(String::as_str), &admitting, &mut streams.proposals)
            .map_err(|e| format!("{e}"))?;
    let landed = attribute_heard(context, seat.identity, &progress.along, &venues, streams)?;
    seat.proposed.extend(landed);
    progress.admitted += venues.len();
    progress.refused += refused.len();
    // Advanced only on an answer, so a slow strategy's decision is not refused for naming an event
    // it was still working on.
    if !heard.lines.is_empty() {
        for along in &mut progress.along {
            along.batch_from = along.through.map_or(along.batch_from, |t| t.saturating_add(1));
        }
    }
    for line in &refused {
        eprintln!(
            "{PROGRAM}: {}: line {} refused: {}",
            seat.program.display(),
            line.line,
            line.because
        );
    }
    if heard.closed {
        seat.ended = Some("the strategy closed its output".to_string());
    }
    Ok(())
}

/// Records which proposal positions the admitted proposals landed on, each stretch against the
/// market of the venue it names, and returns those positions.
fn attribute_heard(
    context: &Context<'_>,
    specification: Digest,
    along: &[Along],
    venues: &[Option<String>],
    streams: &mut Streams,
) -> Result<std::ops::Range<u64>, String> {
    let end = streams.proposals.next_sequence();
    let mut from = end - venues.len() as u64;
    let start = from;
    let feeds: Vec<&markets::Feed> = context.sources.iter().map(|s| &s.feed).collect();
    for (s, count) in markets::stretches(venues, &feeds) {
        let landed = Admitted {
            specification,
            from,
            through: from + count - 1,
            count,
            market: context.sources[s].feed.stream_name.clone(),
            market_through: along[s].through.unwrap_or(along[s].resume_from),
            proposals: context.stage.proposals().to_string(),
        };
        attribute(&landed, &mut streams.ledger).map_err(|e| format!("{e}"))?;
        from += count;
    }
    Ok(start..end)
}

/// Sends each strategy what became of its own proposals since the last poll. A record about a
/// proposal no strategy of this session made is not anyone's here and is passed over. One that
/// cannot be read ends the session, since a strategy told less than happened trades on a guess.
fn tell(store: &Path, seats: &mut [Seat], listening: &mut Listening) -> Result<(), String> {
    let (entries, cursor) = read_after(store, &listening.feedback_stream, listening.cursor)
        .map_err(|e| format!("the order feedback could not be read: {e:?}"))?;
    listening.cursor = cursor.or(listening.cursor);
    for entry in entries {
        let told = decode_feedback(&entry.payload)
            .map_err(|e| format!("order feedback at {} could not be read: {e:?}", entry.seq))?;
        let owner = seats
            .iter_mut()
            .find(|s| s.ended.is_none() && s.proposed.contains(&told.proposal_position));
        let Some(seat) = owner else { continue };
        let line = to_line(&order_line(&told)).map_err(|e| format!("{e}"))?;
        if let Err(e) = seat.strategy.send(line.as_bytes()) {
            seat.ended = Some(format!("it could not be told what became of its proposal: {e}"));
        }
    }
    Ok(())
}
