// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Replayed sources: a recording of frames, or another store's frame journal. Both run through
//! the recorder exactly as live frames do, so a drill and a production run are the same run.

use std::path::{Path, PathBuf};

use bijective_common::readiness::{Health, Surface};
use bijective_data_engine::journal::{self, JournalEntry};
use bijective_data_engine::recorder::{BATCH_LIMIT, Recorder};
use bijective_event_store::retention::horizon;
use bijective_event_store::store::{end_of, read_range};

use bijective_venue::profile::InstrumentScales;

use crate::args::Args;
use crate::{PROGRAM, detail, die, hold, now_ns};

/// Entries read per call, so a long journal is replayed in bounded memory.
const JOURNAL_BATCH: u64 = 10_000;

/// The arrival time is the recorder's, written as a decimal string because it is an `i128`.
/// A line without one is refused: a replay would otherwise have to invent when it arrived.
pub(crate) fn extract_frame(line: &str) -> Option<(i128, String)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let received_at_ns = value.get("at_ns")?.as_str()?.parse().ok()?;
    Some((received_at_ns, value.get("frame")?.as_str()?.to_string()))
}

/// What every source reads with: where to report, what was asked, and each instrument's scales
/// from the profile (decision 0007).
#[derive(Clone, Copy)]
pub(crate) struct Read<'a> {
    pub(crate) surface: &'a Surface,
    pub(crate) args: &'a Args,
    pub(crate) scales: &'a InstrumentScales,
}

/// A recording holds frames only, so this run supplies its own session marks.
pub(crate) async fn run_recording(
    read: &Read<'_>,
    mut recorder: Recorder<'_>,
    path: &PathBuf,
) -> std::process::ExitCode {
    let Read { surface, args, scales } = *read;
    let mut feed = match bijective_venue_registry::open_market_feed(&args.venue, scales) {
        Ok(f) => f,
        Err(e) => return die(surface, format!("venue refused: {e}")),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return die(surface, format!("recording unreadable: {e}")),
    };

    if let Err(e) = recorder.session_started(now_ns()) {
        return die(surface, e.to_string());
    }
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((received_at_ns, frame)) = extract_frame(line) else {
            return die(surface, format!("recording line {n} carries no frame and arrival time"));
        };
        let entry = JournalEntry::Frame { received_at_ns, frame: frame.into_bytes() };
        if let Err(e) = recorder.replay(&entry, feed.as_mut()) {
            return die(surface, e.to_string());
        }
        if (n + 1) % BATCH_LIMIT == 0
            && let Err(e) = recorder.commit()
        {
            return die(surface, e.to_string());
        }
    }
    if let Err(e) = recorder.session_ended(now_ns()) {
        return die(surface, e.to_string());
    }
    let place = format!("across {} instrument(s)", feed.instruments());
    finish(surface, args, recorder, &place).await
}

/// A journal carries its own session marks. They are replayed, not added to.
pub(crate) async fn run_journal(
    read: &Read<'_>,
    mut recorder: Recorder<'_>,
    root: &Path,
) -> std::process::ExitCode {
    let Read { surface, args, scales } = *read;
    let mut feed = match bijective_venue_registry::open_market_feed(&args.venue, scales) {
        Ok(f) => f,
        Err(e) => return die(surface, format!("venue refused: {e}")),
    };
    let stream = journal::stream(&args.stream_name);
    let last = match end_of(root, &stream) {
        Ok(Some(last)) => last,
        Ok(None) => return die(surface, format!("{} holds no {stream} journal", root.display())),
        Err(e) => return die(surface, format!("the journal could not be read: {e:?}")),
    };

    // A pruned journal begins at its horizon. The archive holds what came before it.
    let mut from = match horizon(root, &stream) {
        Ok(h) => h.map_or(0, |h| h.first_seq),
        Err(e) => return die(surface, format!("the journal's horizon could not be read: {e}")),
    };
    while from <= last {
        let to = last.min(from + JOURNAL_BATCH - 1);
        let entries = match read_range(root, &stream, from, to) {
            Ok(e) => e,
            Err(e) => return die(surface, format!("the journal could not be read: {e:?}")),
        };
        for (n, stored) in entries.into_iter().enumerate() {
            let replayed = journal::decode(&stored.payload)
                .map_err(|e| format!("journal entry {} refused: {e:?}", stored.seq))
                .and_then(|entry| {
                    recorder.replay(&entry, feed.as_mut()).map_err(|e| e.to_string())
                });
            if let Err(e) = replayed {
                return die(surface, e);
            }
            if (n + 1) % BATCH_LIMIT == 0
                && let Err(e) = recorder.commit()
            {
                return die(surface, e.to_string());
            }
        }
        from = to + 1;
    }
    let place = format!("across {} instrument(s)", feed.instruments());
    finish(surface, args, recorder, &place).await
}

async fn finish(
    surface: &Surface,
    args: &Args,
    recorder: Recorder<'_>,
    place: &str,
) -> std::process::ExitCode {
    let counts = recorder.counts();
    // Reported after sealing, so a reader of `ready` finds the archive complete.
    let archive = match recorder.finish(now_ns()) {
        Ok(a) => a,
        Err(e) => return die(surface, format!("the archive could not be sealed: {e}")),
    };
    let text = detail(counts, archive, place);
    eprintln!("{PROGRAM}: {text}");
    surface.report_or_warn(Health::Ready, &text, now_ns());
    hold(surface, &text, args.drill_seconds).await
}
