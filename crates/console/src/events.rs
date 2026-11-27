// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The console's tables as Server-Sent Events: every table first, then each again when it changes.
//!
//! Each event stream holds its own fold on its own thread, so the request loop never waits on a
//! reader and needs no lock. The number open at once is bounded: the node's CPU is for trading.

use std::collections::BTreeMap;
use std::io::{Result, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bijective_common::clock::{Clock, WallClock};

use crate::http::{JSON, open_events, respond_as, send_event, send_keep_alive};
use crate::panels::Standing;
use crate::{Console, Streams};

/// Event streams open at once. One more is refused with a 503 and opens nothing.
pub const MAX_EVENT_STREAMS: usize = 4;
/// How often a stream refreshes its fold. A page reloads every ten seconds.
const REFRESH_EVERY: Duration = Duration::from_secs(1);

static OPEN: AtomicUsize = AtomicUsize::new(0);

/// What an event stream reads, owned, so its thread outlives the request that opened it.
#[derive(Debug, Clone)]
pub struct Sources {
    pub store_root: PathBuf,
    pub archive: Option<PathBuf>,
    /// The manifest and the run directory, as the console was given them.
    pub standing: Option<(PathBuf, PathBuf)>,
    pub streams: Streams,
}

impl Sources {
    #[must_use]
    pub fn standing(&self) -> Standing {
        match &self.standing {
            Some((manifest, run_dir)) => {
                Standing::read(manifest, run_dir, WallClock::new().now_ns())
            }
            None => Standing::NotConfigured,
        }
    }
}

/// Gives its place back when the stream's thread ends, however it ends.
struct Place;

impl Drop for Place {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Answers an event stream request. The thread is not joined: it ends when its peer leaves or
/// stops reading for the connection's write timeout, and `MAX_EVENT_STREAMS` bounds how many live.
pub fn subscribe<S: Write + Send + 'static>(mut stream: S, sources: &Sources) -> Result<()> {
    if OPEN.fetch_add(1, Ordering::SeqCst) >= MAX_EVENT_STREAMS {
        OPEN.fetch_sub(1, Ordering::SeqCst);
        let body = "{\"error\":\"every event stream is taken\"}";
        return respond_as(&mut stream, "503 Service Unavailable", JSON, body);
    }
    let place = Place;
    let sources = sources.clone();
    std::thread::Builder::new()
        .name("console-events".into())
        .spawn(move || {
            let _place = place;
            if let Err(e) = stream_events(stream, &sources) {
                eprintln!("bijective-console: an event stream ended: {e}");
            }
        })
        .map(|_| ())
}

/// Never returns `Ok`: the stream runs until a write fails. A pass with no change still writes a
/// comment of a few bytes, because only a write finds a peer that left, and its place is free
/// again only when it is found.
fn stream_events(mut stream: impl Write, sources: &Sources) -> Result<()> {
    open_events(&mut stream)?;
    let mut console = Console::default();
    let mut sent = BTreeMap::new();
    loop {
        console.refresh(
            &sources.store_root,
            &sources.streams,
            sources.archive.as_deref(),
            sources.standing(),
        );
        let changed = console.changed(&mut sent);
        for (name, document) in &changed {
            send_event(&mut stream, name, document)?;
        }
        if changed.is_empty() {
            send_keep_alive(&mut stream)?;
        } else {
            stream.flush()?;
        }
        std::thread::sleep(REFRESH_EVERY);
    }
}
