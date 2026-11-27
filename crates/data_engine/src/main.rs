// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The Data Engine program: journals venue frames, normalises them through the registry, archives
//! the records and appends the top of book to the store. It holds no credential, cannot halt
//! trading and names no venue.

use std::sync::Arc;
use std::time::Duration;

use bijective_archive::{ArchiveConfig, ArchiveCounts};
use bijective_common::clock::{Clock, WallClock};
use bijective_common::readiness::{Health, Surface};
use bijective_data_engine::journal;
use bijective_data_engine::recorder::{Outputs, Recorder, RecorderCounts};
use bijective_event_store::store::StreamWriter;
use bijective_venue::profile::{InstrumentCatalogue, InstrumentScales, VenueProfile};

mod account;
mod args;
mod live;
mod replay;
mod stop;

use args::{Args, Source, parse_args};

const PROGRAM: &str = "data-engine";

const HEARTBEAT: Duration = Duration::from_secs(5);

/// One clock per program: a direct read would opt out of reproducibility.
fn clock() -> &'static Arc<WallClock> {
    static CLOCK: std::sync::OnceLock<Arc<WallClock>> = std::sync::OnceLock::new();
    CLOCK.get_or_init(|| Arc::new(WallClock::new()))
}

fn now_ns() -> i128 {
    clock().now_ns()
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    // Before the first report: a stop sent once the program reads as ready must find a handler.
    if let Err(e) = stop::install() {
        eprintln!(
            "{PROGRAM}: no stop handler could be registered, so a stop ends the process: {e}"
        );
    }

    let surface = Surface::new(&args.readiness, PROGRAM);
    // The first report is checked: a node without a readiness document looks the same as a healthy one.
    if let Err(e) = surface.report(Health::Starting, "opening the event store", now_ns()) {
        eprintln!("{PROGRAM}: the readiness surface could not be written: {e}");
        return std::process::ExitCode::from(2);
    }

    // Reported before the store opens and after: a slow disk leaves a program that looks alive and does nothing.
    let mut market = match StreamWriter::open(
        &args.store_root,
        &bijective_serialization::signal::market::stream(&args.stream_name),
    ) {
        Ok(s) => s,
        Err(e) => return die(&surface, format!("store refused to open: {e:?}")),
    };
    let mut facts = match StreamWriter::open(
        &args.store_root,
        &bijective_serialization::signal::facts::stream(&args.stream_name),
    ) {
        Ok(s) => s,
        Err(e) => return die(&surface, format!("the facts stream refused to open: {e:?}")),
    };
    let journal = match StreamWriter::open(&args.store_root, &journal::stream(&args.stream_name)) {
        Ok(s) => match args.journal_segment_bytes {
            Some(bytes) => s.with_max_segment_bytes(bytes),
            None => s,
        },
        Err(e) => return die(&surface, format!("the frame journal refused to open: {e:?}")),
    };

    // Loaded before ingest: without the catalogue every frame would be refused one at a time.
    let catalogue = match load_catalogue(&args) {
        Ok(c) => c,
        Err(e) => return die(&surface, e),
    };
    // Declared before ingest, so a strategy session opened on this venue is told its increments.
    if let Err(e) = bijective_data_engine::declared::declare_instruments(
        &args.store_root,
        &args.stream_name,
        &catalogue,
    ) {
        return die(&surface, format!("the instrument rules could not be declared: {e}"));
    }

    let scales = match InstrumentScales::of_catalogue(&catalogue) {
        Ok(s) => s,
        Err(e) => return die(&surface, format!("the profile's precision is unusable: {e}")),
    };

    let outputs = Outputs { market: &mut market, facts: &mut facts };
    let recorder = match open_recorder(&args, journal, outputs, &catalogue, &scales) {
        Ok(r) => r,
        Err(e) => return die(&surface, e),
    };

    let read = replay::Read { surface: &surface, args: &args, scales: &scales };
    match args.source {
        Source::Recording(ref path) => replay::run_recording(&read, recorder, path).await,
        Source::Journal(ref root) => replay::run_journal(&read, recorder, root).await,
        Source::Live { ref endpoint, ref instruments } => {
            live::run_live(&read, recorder, endpoint, instruments).await
        }
    }
}

fn load_catalogue(args: &Args) -> Result<InstrumentCatalogue, String> {
    let text =
        std::fs::read_to_string(&args.profile).map_err(|e| format!("profile unreadable: {e}"))?;
    let profile = VenueProfile::from_json(&text).map_err(|e| format!("profile refused: {e}"))?;
    Ok(profile.instruments)
}

/// An archive that was not closed is reported, not repaired: its last rows are in the journal,
/// and an operator rebuilds with `--replay-store`.
fn open_recorder<'a>(
    args: &Args,
    journal: StreamWriter,
    outputs: Outputs<'a>,
    catalogue: &'a InstrumentCatalogue,
    scales: &InstrumentScales,
) -> Result<Recorder<'a>, String> {
    let archive = match args.archive.as_ref() {
        None => None,
        // The archive writes one scale per venue directory (decision 0007, rule 3).
        Some(root) => {
            let one = scales.uniform().map_err(|e| format!("the archive is refused: {e}"))?;
            let config = ArchiveConfig {
                root: root.clone(),
                venue: args.stream_name.clone(),
                price_scale: one.price.digits(),
                quantity_scale: one.quantity.digits(),
            };
            Some((config, now_ns()))
        }
    };
    let (recorder, report) = Recorder::new(journal, outputs, catalogue, archive)
        .map_err(|e| format!("recorder refused to open: {e}"))?;
    if let Some(report) = report {
        if !report.previous_session_closed {
            eprintln!(
                "{PROGRAM}: the archive's previous session did not close; rebuild it from the journal"
            );
        }
        for path in &report.quarantined {
            eprintln!("{PROGRAM}: an unlisted archive file was set aside: {path}");
        }
    }
    Ok(recorder)
}

/// Every count in one sentence. The first clause is what drills read.
fn detail(counts: RecorderCounts, archive: Option<ArchiveCounts>, place: &str) -> String {
    let archived = match archive {
        Some(a) => format!(
            "{} archive row(s) sealed in {} file(s), {} buffered, {} record(s) with no archive \
             table yet (in the journal)",
            a.sealed_rows, a.sealed_files, a.buffered_rows, a.not_archived
        ),
        None => format!("{} market record(s) carried, not archived", counts.records),
    };
    format!(
        "ingested {} book event(s) {place}; {} frame(s) journaled; {} gap mark(s); {} fact(s) \
         for strategies, {} on undeclared symbols; {archived}",
        counts.tops, counts.frames, counts.gaps, counts.facts, counts.undeclared_facts
    )
}

// Idling still re-stamps readiness. A program that stops is reported stale, not healthy.
async fn hold(
    surface: &Surface,
    detail: &str,
    drill_seconds: Option<u64>,
) -> std::process::ExitCode {
    let deadline = drill_seconds.map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.tick().await;

    loop {
        tokio::select! {
            _ = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => return std::process::ExitCode::SUCCESS,
            () = stop::requested() => {
                eprintln!("{PROGRAM}: stop requested; the archive was already sealed");
                return std::process::ExitCode::SUCCESS;
            }
            _ = beat.tick() => {
                surface.report_or_warn(Health::Ready, detail, now_ns());
            }
        }
    }
}

fn die(surface: &Surface, why: String) -> std::process::ExitCode {
    surface.report_or_warn(Health::Failed, &why, now_ns());
    eprintln!("{PROGRAM}: {why}");
    std::process::ExitCode::FAILURE
}

#[cfg(test)]
#[path = "../tests/unit/recording_tests.rs"]
mod recording_tests;
