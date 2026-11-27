// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The console: one operator surface whose panels are grouped behind routes.
//!
//! Every figure is a fold over the core's streams and nothing here is authoritative: losing the
//! console degrades visibility, never function. Each request reads what each stream gained since
//! the last, decodes each entry once, and offers it to every panel (`panel::Panel`). Which panels
//! exist and how they are grouped is the one list in `panels::groups`. See
//! `docs/concepts/observability.md`.

pub mod app;
pub mod control;
pub mod desk;
pub mod detection;
pub mod events;
pub mod http;
pub mod markup;
pub mod metrics;
pub mod panel;
pub mod panels;
pub mod performance;
pub mod table;
pub mod tls;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bijective_archive::manifest::MANIFEST;
use bijective_archive::newest_fact_rows;
use bijective_control_engine::wire::decode as control;
use bijective_data_engine::link::decode as link_transition;
use bijective_event_store::cursor::{Cursor, read_after};
use bijective_event_store::format::Entry as StoreEntry;
use bijective_event_store::store::{StoreError, read_tail};
use bijective_risk::limit_state::wire::decode as limit;
use bijective_serialization::signal::command::decode as command;
use bijective_serialization::signal::market::{decode as book, venue_of};

use crate::desk::Desk;
use crate::panel::{Entry, Signal, Unreadable};
use crate::panels::{Group, Standing};

/// The names of the streams the console reads. The link streams are found by their prefix.
#[derive(Debug, Clone)]
pub struct Streams {
    /// The run mode the streams belong to, carried on the strip on every page.
    pub mode: String,
    pub control: String,
    pub limits: String,
    pub commands: String,
    /// Read only where it exists: a node with no detector is not broken.
    pub detections: String,
    /// Every run mode's performance stream, by mode, read where it exists: one strategy's runs
    /// are compared side by side (decision 0017 stage 2), while commands stay with `mode`.
    pub performance: Vec<(String, String)>,
}

impl Default for Streams {
    fn default() -> Self {
        Self::of(bijective_serialization::signal::node::LIVE)
    }
}

impl Streams {
    /// The streams of one run mode's node, and the detections written about it.
    #[must_use]
    pub fn of(node: bijective_serialization::signal::node::NodeStreams) -> Self {
        Self {
            mode: node.mode.into(),
            control: node.control.into(),
            limits: node.limits.into(),
            commands: node.commands.into(),
            detections: detection::stream_of(node.mode),
            performance: performance::every_mode(),
        }
    }
}

const LINK_PREFIX: &str = "link-";
/// Where `bijective-backtest` keeps each backtest's record, below the store it ran over.
const BACKTEST_RECORDS: &str = "run/backtests";
const MARKET_PREFIX: &str = "market-";
/// Book tops read per market stream. A market stream grows by millions of entries a day. The
/// console reads its newest entries only, never its history.
pub const MARKET_TAIL: usize = 300;
/// Fact rows read per venue from the archive's newest files, about a few minutes of a busy feed.
/// Bounded by rows, not files: one file of a compacted day can hold millions.
pub const FACT_ROWS: u64 = 50_000;

/// The panels and where each source was last read to. Kept between requests, so a request reads
/// and decodes only what was appended since the one before.
pub struct Console {
    pub groups: Vec<Group>,
    /// By stream name: where its last read ended.
    cursors: BTreeMap<String, Cursor>,
    /// By market stream: the newest entry offered.
    books_offered: BTreeMap<String, u64>,
    /// By archive venue: the newest row offered, as journal entry and record within it.
    facts_offered: BTreeMap<String, (u64, u32)>,
    /// Sources whose last read failed, so a failure is listed once and not on every request.
    failing: BTreeSet<String>,
    /// A stream was rewritten under a cursor: the fold describes a history that no longer exists.
    rewritten: bool,
    /// Who this console signs commands as. `None` issues nothing.
    desk: Option<Desk>,
    /// The run mode last read. Empty before the first refresh.
    mode: String,
    /// Venues seen on a link stream, which a venue command may name.
    venues: BTreeSet<String>,
}

/// What a route answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    Found(String),
    /// No group is behind this route. The page still carries the strip and the navigation.
    NotFound(String),
}

impl Default for Console {
    fn default() -> Self {
        Self {
            groups: panels::groups(),
            cursors: BTreeMap::new(),
            books_offered: BTreeMap::new(),
            facts_offered: BTreeMap::new(),
            failing: BTreeSet::new(),
            rewritten: false,
            desk: None,
            mode: String::new(),
            venues: BTreeSet::new(),
        }
    }
}

impl Console {
    /// A console that signs the commands its control page submits as `desk`'s operator.
    #[must_use]
    pub fn with_desk(desk: Desk) -> Self {
        Self { desk: Some(desk), ..Self::default() }
    }

    #[must_use]
    pub fn desk(&self) -> Option<&Desk> {
        self.desk.as_ref()
    }

    /// Offers the panels what was appended since the last refresh, and the standing as it is now.
    /// Postcondition: the panels hold what one fold over the sources would hold, or, where a
    /// stream was rewritten under the console, the fold starts over. A stream that does not
    /// exist yields nothing: a node with no first limit is not broken.
    pub fn refresh(
        &mut self,
        root: &Path,
        streams: &Streams,
        archive: Option<&Path>,
        standing: Standing,
    ) {
        self.read_on(root, streams, archive, &standing);
        if self.rewritten {
            let desk = self.desk.take();
            *self = Self { desk, ..Self::default() };
            self.read_on(root, streams, archive, &standing);
        }
    }

    fn read_on(
        &mut self,
        root: &Path,
        streams: &Streams,
        archive: Option<&Path>,
        standing: &Standing,
    ) {
        self.mode.clone_from(&streams.mode);
        self.offer(&Entry::Standing(standing));
        if let Some(desk) = self.desk.take() {
            self.offer(&Entry::Desk(&desk));
            self.desk = Some(desk);
        }
        self.fold_control(root, &streams.control);
        self.fold_limits(root, &streams.limits);
        self.fold_commands(root, &streams.commands);
        self.fold_detections(root, &streams.detections);
        for (mode, stream) in &streams.performance {
            self.fold_performance(root, stream, mode, stream);
        }
        self.fold_backtest_runs(root);
        self.fold_links(root);
        self.fold_markets(root);
        if let Some(archive) = archive {
            self.fold_archive(archive);
        }
    }

    fn offer(&mut self, entry: &Entry<'_>) {
        for group in &mut self.groups {
            for panel in &mut group.panels {
                panel.apply(entry);
            }
        }
    }

    fn unreadable(&mut self, stream: &'static str, position: u64) {
        self.offer(&Entry::Unreadable(&Unreadable { stream, position }));
    }

    /// Listed when a source starts failing, not again until it has read once more.
    fn failed(&mut self, source: &str, label: &'static str, position: u64) {
        if self.failing.insert(source.to_string()) {
            self.unreadable(label, position);
        }
    }

    /// The entries `stream` gained since its cursor. A failed read leaves the cursor, so the
    /// next request tries the same entries again.
    fn read_new(&mut self, root: &Path, stream: &str, label: &'static str) -> Vec<StoreEntry> {
        self.read_new_as(root, stream, stream, label)
    }

    /// As `read_new`, its cursor kept under `key`: one stream name in several record directories.
    fn read_new_as(
        &mut self,
        root: &Path,
        stream: &str,
        key: &str,
        label: &'static str,
    ) -> Vec<StoreEntry> {
        let from = self.cursors.get(key).copied();
        match read_after(root, stream, from) {
            Ok((entries, cursor)) => {
                if let Some(cursor) = cursor {
                    self.cursors.insert(key.to_string(), cursor);
                }
                self.failing.remove(stream);
                entries
            }
            Err(StoreError::Rewritten { .. }) => {
                self.rewritten = true;
                Vec::new()
            }
            Err(_) => {
                self.failed(stream, label, from.map_or(0, |c| c.next_seq()));
                Vec::new()
            }
        }
    }

    fn fold_control(&mut self, root: &Path, stream: &str) {
        for entry in self.read_new(root, stream, "control") {
            match control(&entry.payload) {
                Ok(event) => self.offer(&Entry::Control(&event)),
                Err(_) => self.unreadable("control", entry.seq),
            }
        }
    }

    fn fold_limits(&mut self, root: &Path, stream: &str) {
        for entry in self.read_new(root, stream, "limits") {
            match limit(&entry.payload) {
                Ok(event) => self.offer(&Entry::Limit(&event)),
                Err(_) => self.unreadable("limits", entry.seq),
            }
        }
    }

    /// Commands are shown through what came of them, on the control stream. This stream is read
    /// only so that an entry nobody can decode is listed.
    fn fold_commands(&mut self, root: &Path, stream: &str) {
        for entry in self.read_new(root, stream, "commands") {
            if command(&entry.payload).is_err() {
                self.unreadable("commands", entry.seq);
            }
        }
    }

    fn fold_detections(&mut self, root: &Path, stream: &str) {
        for entry in self.read_new(root, stream, "detections") {
            match detection::decode(&entry.payload) {
                Ok(found) => self.offer(&Entry::Detection(&found)),
                Err(_) => self.unreadable("detections", entry.seq),
            }
        }
    }

    fn fold_performance(&mut self, root: &Path, key: &str, run: &str, stream: &str) {
        for entry in self.read_new_as(root, stream, key, "performance") {
            match performance::decode(&entry.payload) {
                Ok(found) => self.offer(&Entry::Performance { run, performance: &found }),
                Err(_) => self.unreadable("performance", entry.seq),
            }
        }
    }

    /// The performance of every backtest recorded under this store (`run/backtests/<window>`, where
    /// `bijective-backtest` writes each), named `backtest <window>`, beside the store's own runs.
    /// Sorted, so the fold is the same on every host.
    fn fold_backtest_runs(&mut self, root: &Path) {
        let runs = root.join(BACKTEST_RECORDS);
        let mut windows: Vec<String> = std::fs::read_dir(&runs)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect();
        windows.sort();
        let stream = performance::stream_of("backtest");
        for window in windows {
            let key = format!("{BACKTEST_RECORDS}/{window}/{stream}");
            let run = format!("backtest {window}");
            self.fold_performance(&runs.join(&window), &key, &run, &stream);
        }
    }

    /// Every venue's link history, discovered from stream names so there is no venue list to
    /// get wrong. Sorted, so the fold is the same on every host.
    fn fold_links(&mut self, root: &Path) {
        for name in streams_named(root, LINK_PREFIX) {
            let venue = name.trim_start_matches(LINK_PREFIX).to_string();
            self.venues.insert(venue.clone());
            for entry in self.read_new(root, &name, "link") {
                match link_transition(&entry.payload) {
                    Ok(transition) => {
                        self.offer(&Entry::Link { venue: &venue, transition: &transition });
                    }
                    Err(_) => self.unreadable("link", entry.seq),
                }
            }
        }
    }

    /// The newest book tops of every market stream, discovered by name like the links. Read
    /// from the tail, never from a cursor: a first read from a cursor would read the history.
    fn fold_markets(&mut self, root: &Path) {
        for name in streams_named(root, MARKET_PREFIX) {
            let Some(venue) = venue_of(&name).map(str::to_string) else { continue };
            let Ok(entries) = read_tail(root, &name, MARKET_TAIL) else {
                self.failed(&name, "market", 0);
                continue;
            };
            self.failing.remove(&name);
            let offered = self.books_offered.get(&name).copied();
            if offered.is_some_and(|o| entries.last().is_none_or(|e| e.seq < o)) {
                self.rewritten = true;
                continue;
            }
            for entry in entries.iter().filter(|e| offered.is_none_or(|o| e.seq > o)) {
                match book(&entry.payload) {
                    Ok(update) => self.offer(&Entry::Book { venue: &venue, update: &update }),
                    Err(_) => self.unreadable("market", entry.seq),
                }
            }
            if let Some(newest) = entries.last() {
                self.books_offered.insert(name, newest.seq);
            }
        }
    }

    /// The newest facts of every venue the archive holds, found from its directories like the
    /// streams. A venue whose archive cannot be read is listed as unreadable, never left out.
    fn fold_archive(&mut self, archive: &Path) {
        for venue in venues_in(archive) {
            let source = format!("archive/{venue}");
            let newest = match newest_fact_rows(archive, &venue, FACT_ROWS) {
                Ok(Some(newest)) => newest,
                Ok(None) => continue,
                Err(_) => {
                    self.failed(&source, "archive", 0);
                    continue;
                }
            };
            self.failing.remove(&source);
            let offered = self.facts_offered.get(&venue).copied();
            let newest_row = newest.rows.last().map(|r| (r.journal_seq, r.record_in_frame));
            if offered.is_some_and(|o| newest_row.is_none_or(|n| n < o)) {
                self.rewritten = true;
                continue;
            }
            let fresh = newest
                .rows
                .iter()
                .filter(|r| offered.is_none_or(|o| (r.journal_seq, r.record_in_frame) > o));
            for row in fresh {
                self.offer(&Entry::Fact {
                    venue: &venue,
                    price_scale: newest.price_scale,
                    quantity_scale: newest.quantity_scale,
                    row,
                });
            }
            if let Some(last) = newest.rows.last() {
                self.facts_offered.insert(venue, (last.journal_seq, last.record_in_frame));
            }
        }
    }

    /// The strip: every panel's signals, in navigation order, each once. A panel shown in two
    /// groups folds twice and would otherwise put the same figure on the strip twice.
    #[must_use]
    pub fn signals(&self) -> Vec<Signal> {
        // First, so no reader takes a paper or backtest record for the node that places capital.
        let mut signals: Vec<Signal> = Vec::new();
        if !self.mode.is_empty() {
            let value = self.mode.clone();
            signals.push(Signal { label: "mode", value, level: panel::Level::Normal });
        }
        for signal in self.groups.iter().flat_map(|g| g.panels.iter()).flat_map(|p| p.signals()) {
            if !signals.contains(&signal) {
                signals.push(signal);
            }
        }
        signals
    }

    /// The strip as OpenMetrics text (`metrics::render`).
    #[must_use]
    pub fn metrics(&self) -> String {
        metrics::render(&self.signals())
    }

    /// Every table this console serves, as `(panel, title, document)`: the strip first, then each
    /// panel that offers rows, once, in navigation order.
    #[must_use]
    pub fn documents(&self) -> Vec<(String, &'static str, String)> {
        let strip = table::strip(&self.signals()).json(STRIP, STRIP_TITLE);
        let mut found = vec![(STRIP.to_string(), STRIP_TITLE, strip)];
        for panel in self.groups.iter().flat_map(|g| g.panels.iter()) {
            let name = table::slug(panel.title());
            if found.iter().any(|(n, ..)| *n == name) {
                continue;
            }
            if let Some(rows) = panel.rows() {
                let document = rows.json(&name, panel.title());
                found.push((name, panel.title(), document));
            }
        }
        found
    }

    /// What `API_PANELS` and `API_PANELS/<panel>` answer. `None` where no table is behind the
    /// route.
    #[must_use]
    pub fn api(&self, route: &str) -> Option<String> {
        let route = route.split(['?', '#']).next().unwrap_or("");
        let documents = self.documents();
        if route == API_PANELS {
            return Some(index(&documents));
        }
        if route == API_CONTROL {
            return Some(control::offer(self.desk.as_ref(), &self.mode, &self.venues));
        }
        let name = route.strip_prefix(API_PANELS)?.strip_prefix('/')?;
        documents.into_iter().find(|(n, ..)| n == name).map(|(_, _, document)| document)
    }

    /// The documents that differ from what `sent` holds, which then holds them: what an event
    /// stream sends after a refresh. Against an empty `sent`, every document.
    pub fn changed(&self, sent: &mut BTreeMap<String, String>) -> Vec<(String, String)> {
        let mut changed = Vec::new();
        for (name, _, document) in self.documents() {
            if sent.get(&name) != Some(&document) {
                sent.insert(name.clone(), document.clone());
                changed.push((name, document));
            }
        }
        changed
    }

    /// The page behind `route`, a path with or without its leading slash.
    #[must_use]
    pub fn page(&self, route: &str) -> Page {
        self.page_noting(route, None)
    }

    /// The page behind `route` with `notice`, plain text, above its panels: what came of a form.
    #[must_use]
    pub fn page_noting(&self, route: &str, notice: Option<&str>) -> Page {
        let route = route.trim_start_matches('/');
        let route = route.split(['?', '#']).next().unwrap_or("");
        let nav: Vec<(&'static str, &'static str)> =
            self.groups.iter().map(|g| (g.route, g.label)).collect();
        let signals = self.signals();
        match self.groups.iter().find(|g| g.route == route) {
            Some(group) => {
                let mut body = String::new();
                if let Some(notice) = notice {
                    body.push_str(&format!("<p class=\"attention\">{}</p>", markup::text(notice)));
                }
                for panel in &group.panels {
                    body.push_str(&format!("<h2>{}</h2>", panel.title()));
                    panel.render(&mut body);
                }
                Page::Found(markup::page(group.label, group.route, &nav, &signals, &body))
            }
            None => {
                let body = "<p>No panel is behind this address.</p>";
                Page::NotFound(markup::page("not found", "-", &nav, &signals, body))
            }
        }
    }
}

/// The tables a program reads: the index here, each panel under it.
pub const API_PANELS: &str = "/api/panels";
/// What this console may sign, for an application that offers the same commands as the Control
/// page: never sent on the event stream, answered only to a request of this origin's own.
pub const API_CONTROL: &str = "/api/control";
/// The same tables as Server-Sent Events, each sent again when it changes.
pub const API_EVENTS: &str = "/api/events";
const STRIP: &str = "strip";
const STRIP_TITLE: &str = "Strip";

/// The panels a program can read, by name and title.
fn index(documents: &[(String, &'static str, String)]) -> String {
    let mut out = format!("{{\"format\":{},\"panels\":[", table::FORMAT);
    for (i, (name, title, _)) in documents.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"panel\":");
        table::quoted(&mut out, name);
        out.push_str(",\"title\":");
        table::quoted(&mut out, title);
        out.push('}');
    }
    out.push_str("]}");
    out
}

/// The stream names under `root` that begin with `prefix`, sorted so the fold is the same on every
/// host. An unreadable root names none.
fn streams_named(root: &Path, prefix: &str) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(root) else { return Vec::new() };
    let mut found: Vec<String> = listing
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix))
        .collect();
    found.sort();
    found
}

/// The venues of a market archive: the directories under `archive` that hold a manifest, sorted.
fn venues_in(archive: &Path) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(archive) else { return Vec::new() };
    let mut found: Vec<String> = listing
        .filter_map(Result::ok)
        .filter(|e| e.path().join(MANIFEST).is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    found.sort();
    found
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
