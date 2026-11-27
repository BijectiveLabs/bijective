// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The live source: a venue stream through the registry, every event applied through the
//! recorder in the order the adapter sent it.

use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::Clock;
use bijective_common::readiness::{Health, Surface};
use bijective_data_engine::link::{self, LinkTransition};
use bijective_data_engine::recorder::{BATCH_LIMIT, Recorder, RecorderError};
use bijective_event_store::store::StreamWriter;
use bijective_venue::contract::{ConnectionState, MarketEvent, StreamRequest};
use tokio::sync::mpsc;

use crate::replay::Read;
use crate::{HEARTBEAT, PROGRAM, clock, detail, die, now_ns, stop};

/// Ingests a live venue stream until `--drill-seconds` elapses or the stream ends.
pub(crate) async fn run_live(
    read: &Read<'_>,
    recorder: Recorder<'_>,
    endpoint: &str,
    instruments: &[String],
) -> std::process::ExitCode {
    let Read { surface, args, scales } = *read;
    let credential = match crate::account::credential_for(&args.venue, args.account.as_ref()) {
        Ok(credential) => credential,
        Err(reason) => return die(surface, reason),
    };
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(4096);
    let venue = args.venue.clone();
    let request = StreamRequest {
        endpoint: endpoint.to_string(),
        instruments: instruments.to_vec(),
        scales: scales.clone(),
        clock: Arc::clone(clock()) as Arc<dyn Clock>,
    };
    tokio::spawn(async move {
        // The receiver only sees the channel close. The cause is reported here so "the venue
        // stream ended" in the readiness detail has an explanation in the log.
        if let Err(e) =
            bijective_venue_registry::run_market_stream(&venue, &request, credential, tx).await
        {
            eprintln!("{PROGRAM}: the venue stream ended with an error: {e:?}");
        }
    });

    surface.report_or_warn(Health::Starting, "connecting to the venue", now_ns());
    // A separate stream: book history and link history have different readers.
    let links = match StreamWriter::open(&args.store_root, &link::stream(&args.stream_name)) {
        Ok(w) => w,
        Err(e) => return die(surface, format!("the link stream refused to open: {e:?}")),
    };
    let mut ingest = Ingest { recorder, links, usable: false, last_reported: None };
    if let Err(e) = ingest.recorder.session_started(now_ns()) {
        return die(surface, e.to_string());
    }

    let deadline = args.drill_seconds.map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.tick().await;
    loop {
        tokio::select! {
            _ = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => return ingest.end(surface),

            // A stop is a clean end: the session is marked ended and the archive sealed.
            () = stop::requested() => return ingest.end(surface),

            _ = beat.tick() => {
                let counts = ingest.recorder.counts();
                let text = detail(counts, ingest.recorder.archive_counts(), "from the venue");
                let health = if ingest.usable { Health::Ready } else { Health::Starting };
                surface.report_or_warn(health, text, now_ns());
            }

            event = rx.recv() => {
                let Some(event) = event else {
                    return die(surface, "the venue stream ended".into());
                };
                if let Err(e) = ingest.apply_batch(surface, event, &mut rx) {
                    return die(surface, e.to_string());
                }
            }
        }
    }
}

/// What a live ingest carries between venue events.
struct Ingest<'a> {
    recorder: Recorder<'a>,
    links: StreamWriter,
    usable: bool,
    last_reported: Option<ConnectionState>,
}

impl Ingest<'_> {
    /// Applies `first` and whatever else is already waiting, up to `BATCH_LIMIT` events, then
    /// commits once. Nothing waits for more to arrive: an idle stream commits each frame.
    fn apply_batch(
        &mut self,
        surface: &Surface,
        first: MarketEvent,
        rx: &mut mpsc::Receiver<MarketEvent>,
    ) -> Result<(), RecorderError> {
        self.apply(surface, first)?;
        for _ in 1..BATCH_LIMIT {
            let Ok(event) = rx.try_recv() else { break };
            self.apply(surface, event)?;
        }
        self.recorder.commit()
    }

    /// `Err` stops the program: an entry that cannot be written leaves an unmarked gap in the record.
    fn apply(&mut self, surface: &Surface, event: MarketEvent) -> Result<(), RecorderError> {
        match event {
            MarketEvent::Received(frame) => {
                self.recorder.frame(frame.received_at_ns, &frame.frame)?
            }
            MarketEvent::Record(record) => self.recorder.record(&record)?,
            MarketEvent::Observation(top) => self.recorder.top(&top)?,
            MarketEvent::State(state) => {
                self.usable = state.is_usable();
                surface.report_or_warn(
                    if self.usable { Health::Ready } else { Health::Starting },
                    format!("link {state:?}"),
                    now_ns(),
                );
                self.record_link(state);
            }
            // Reported, never swallowed: an unreadable venue message is a fact missing-event detection needs.
            MarketEvent::Refused(e) => {
                eprintln!("{PROGRAM}: venue frame refused: {e}");
                self.recorder.refused(&e)?;
            }
            MarketEvent::ContinuityLost => {
                self.usable = false;
                surface.report_or_warn(
                    Health::Starting,
                    "continuity lost; books dropped and the subscription is rebuilding",
                    now_ns(),
                );
                self.recorder.continuity_lost(now_ns())?;
            }
        }
        Ok(())
    }

    /// On change only: readiness carries the present, the link stream carries the history.
    /// Never fatal: losing link history is no reason to stop ingesting.
    fn record_link(&mut self, state: ConnectionState) {
        if self.last_reported.as_ref() == Some(&state) {
            return;
        }
        let transition = LinkTransition { state: state.clone(), at_ns: now_ns() };
        let written = link::encode(&transition)
            .map_err(|e| format!("{e:?}"))
            .and_then(|bytes| self.links.append(&bytes).map(|_| ()).map_err(|e| format!("{e:?}")));
        match written {
            Ok(()) => self.last_reported = Some(state),
            Err(e) => eprintln!("{PROGRAM}: a link transition was not recorded: {e}"),
        }
    }

    /// The drill deadline and a stop request are a clean end: the session is marked ended and the
    /// archive sealed before the program exits.
    fn end(self, surface: &Surface) -> std::process::ExitCode {
        let Ingest { mut recorder, .. } = self;
        eprintln!("{PROGRAM}: ending the session and sealing the archive");
        if let Err(e) = recorder.session_ended(now_ns()) {
            return die(surface, e.to_string());
        }
        match recorder.finish(now_ns()) {
            Ok(_) => std::process::ExitCode::SUCCESS,
            Err(e) => die(surface, format!("the archive could not be sealed: {e}")),
        }
    }
}
