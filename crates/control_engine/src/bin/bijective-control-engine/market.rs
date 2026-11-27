// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Marks and the decision-capture book, read incrementally from the market streams, and the
//! positions each venue reports.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use bijective_decision_capture::{
    BookUpdate as CaptureUpdate, Instant, Resolution, SnapshotBuffer,
};
use bijective_event_store::cursor::{Cursor, read_after};
use bijective_event_store::format::Entry;
use bijective_model::market::MarketFact;
use bijective_model::{HeldPosition, InstrumentId};
use bijective_risk::valuation::Mark;
use bijective_venue::contract::contain;
use bijective_venue::profile::Instrument;

use crate::paper::Paper;
use crate::venues::OpenVenues;

/// How far a backtest pass reads (decision 0015): the driving stream through a position, every
/// other market and facts stream through an instant, the proposals through a position.
#[derive(Debug, Clone)]
pub(crate) struct Bound {
    pub(crate) market: String,
    pub(crate) through: u64,
    pub(crate) through_ns: i128,
    pub(crate) proposals_through: Option<u64>,
}

/// The instant a book update reached this node, or its venue time where no receipt was recorded.
pub(crate) fn received_at_ns(update: &bijective_serialization::signal::market::BookUpdate) -> i128 {
    update.received_at_ns.unwrap_or(i128::from(update.venue_ts_ms) * 1_000_000)
}

/// Whether a pass may read `entry` of market `stream` yet. Always, outside a backtest.
fn admits_market(bound: Option<&Bound>, stream: &str, entry: &Entry) -> bool {
    let Some(bound) = bound else { return true };
    if stream == bound.market {
        return entry.seq <= bound.through;
    }
    // An unreadable entry is read and skipped where it lies, as outside a backtest.
    bijective_serialization::signal::market::decode(&entry.payload)
        .map_or(true, |update| received_at_ns(&update) <= bound.through_ns)
}

/// Whether a pass may read `entry` of a facts stream yet. Always, outside a backtest.
fn admits_fact(bound: Option<&Bound>, entry: &Entry) -> bool {
    let Some(bound) = bound else { return true };
    bijective_serialization::signal::facts::decode(&entry.payload)
        .map_or(true, |update| update.received_at_ns <= bound.through_ns)
}

/// The market as this node has read it, kept across passes. Marks come from the data engine's
/// records, so every judged figure traces to a stored entry.
pub(crate) struct MarketView {
    pub(crate) marks: BTreeMap<InstrumentId, Mark>,
    pub(crate) book: SnapshotBuffer,
    /// Where the last read of each stream ended. Read on from there, so a pass verifies only what
    /// was appended since, not the stream's whole history.
    cursors: BTreeMap<String, Cursor>,
    /// Entries read but past a backtest pass's bound, oldest first, for a later pass.
    held_back: BTreeMap<String, VecDeque<Entry>>,
    /// Entries read in the last pass. Shows whether reading stays bounded.
    pub(crate) absorbed: usize,
    /// Outcomes whose question resolved, and whether each won (decision 0002, rule 5).
    pub(crate) resolved: BTreeMap<InstrumentId, bool>,
}

impl MarketView {
    pub(crate) fn new(capacity: usize, max_age_ns: i128) -> Self {
        Self {
            marks: Default::default(),
            book: SnapshotBuffer::new(capacity, max_age_ns),
            cursors: Default::default(),
            held_back: Default::default(),
            absorbed: 0,
            resolved: Default::default(),
        }
    }

    /// The entries appended to `stream` since the last read that `passes` lets through, in order.
    /// The first it holds back, and every entry after it, wait for a later read. `None` when the
    /// stream cannot be read now, which leaves its marks as old as they are, for the freshness
    /// bound to judge.
    fn read_on(
        &mut self,
        store_root: &std::path::Path,
        stream: &str,
        passes: impl Fn(&Entry) -> bool,
    ) -> Option<Vec<Entry>> {
        let (entries, cursor) =
            read_after(store_root, stream, self.cursors.get(stream).copied()).ok()?;
        if let Some(cursor) = cursor {
            self.cursors.insert(stream.to_string(), cursor);
        }
        let waiting = self.held_back.entry(stream.to_string()).or_default();
        waiting.extend(entries);
        let mut admitted = Vec::new();
        while waiting.front().is_some_and(&passes) {
            admitted.extend(waiting.pop_front());
        }
        Some(admitted)
    }

    /// Reads the facts streams for resolutions, then marks every resolved outcome at what it
    /// redeems for (decision 0002, rule 5): one unit of collateral at the instrument's price scale
    /// when it won, nothing when it lost. The mark is restamped each pass, because a settled value
    /// does not age. An outcome the catalogue does not list is left unmarked.
    pub(crate) fn absorb_facts(
        &mut self,
        store_root: &std::path::Path,
        streams: &[String],
        now_ns: i128,
        catalogue: &BTreeMap<InstrumentId, Instrument>,
        bound: Option<&Bound>,
    ) {
        for stream in streams {
            let passes = |entry: &Entry| admits_fact(bound, entry);
            let Some(entries) = self.read_on(store_root, stream, passes) else { continue };
            for entry in entries {
                self.absorbed += 1;
                let Ok(update) = bijective_serialization::signal::facts::decode(&entry.payload)
                else {
                    continue;
                };
                if let MarketFact::Resolution { won } = update.fact {
                    self.resolved.insert(update.instrument, won);
                }
            }
        }
        for (instrument, won) in &self.resolved {
            let Some(listed) = catalogue.get(instrument) else { continue };
            let Some(payout) = 10i128.checked_pow(u32::from(listed.precision.price_scale)) else {
                continue;
            };
            let price_scaled = if *won { payout } else { 0 };
            self.marks.insert(instrument.clone(), Mark { price_scaled, observed_at_ns: now_ns });
        }
    }

    /// Reads every entry written since the last pass, and no earlier entry.
    pub(crate) fn absorb(
        &mut self,
        store_root: &std::path::Path,
        streams: &[String],
        now_ns: i128,
        paper: &Paper,
        bound: Option<&Bound>,
    ) {
        self.absorbed = 0;
        for stream in streams {
            let passes = |entry: &Entry| admits_market(bound, stream, entry);
            let Some(entries) = self.read_on(store_root, stream, passes) else { continue };
            for entry in entries {
                self.absorbed += 1;
                let Ok(update) = bijective_serialization::signal::market::decode(&entry.payload)
                else {
                    continue;
                };
                self.take(&update, now_ns);
                paper.feed(stream, &update);
            }
        }
    }

    fn take(&mut self, update: &bijective_serialization::signal::market::BookUpdate, now_ns: i128) {
        let observed_at_ns = i128::from(update.venue_ts_ms) * 1_000_000;
        // The decision benchmark is keyed on when this host received the book, the clock a
        // strategy's decision instant is read on. The venue's stamp would admit look-ahead.
        let received_ns = received_at_ns(update);
        let resolution = if update.received_at_ns.is_some() {
            Resolution::Nanosecond
        } else {
            Resolution::Millisecond
        };

        // Mid: valuing at the crossing side is an execution decision, not a statement of holdings.
        // A resolved outcome keeps its payout mark whatever a book still says.
        let mid = (update.bid_price + update.ask_price) / 2;
        if !self.resolved.contains_key(&update.instrument) {
            self.marks
                .insert(update.instrument.clone(), Mark { price_scaled: mid, observed_at_ns });
        }

        // A figure wider than i64 is not narrowed: a missing snapshot is visible, a truncated price
        // is not.
        let (Ok(bid), Ok(bid_qty), Ok(ask), Ok(ask_qty)) = (
            i64::try_from(update.bid_price),
            i64::try_from(update.bid_quantity),
            i64::try_from(update.ask_price),
            i64::try_from(update.ask_quantity),
        ) else {
            return;
        };

        self.book.on_market_data(
            CaptureUpdate {
                instrument: update.instrument.as_str().to_string(),
                best_bid_ticks: bid,
                best_bid_qty_lots: bid_qty,
                best_ask_ticks: ask,
                best_ask_qty_lots: ask_qty,
                observed_at: Instant::new(received_ns, resolution),
            },
            now_ns,
        );
    }
}

/// Positions reported by every reachable venue, the venues that could not be read, and the oldest
/// observation time among the venues that were read (0 when none was).
pub(crate) struct Holdings {
    pub(crate) held: Vec<HeldPosition>,
    pub(crate) withdrawn: Vec<String>,
    pub(crate) observed_at_ns: i128,
    /// Each venue read, with the margin its account states, if it states one.
    pub(crate) margins: Vec<(String, Option<bijective_model::AccountMargin>)>,
}

/// Asks every venue what it holds. A venue that cannot be read is withdrawn, never treated as flat.
/// An adapter that panics is marked faulted and is not called again for the life of the process.
pub(crate) async fn read_positions(
    adapters: &mut OpenVenues,
    faulted: &mut BTreeSet<String>,
) -> Holdings {
    let mut held = Vec::new();
    let mut withdrawn = Vec::new();
    let mut oldest = i128::MAX;
    let mut margins = Vec::new();

    for (venue, adapter) in adapters.iter_mut() {
        if faulted.contains(venue) {
            withdrawn.push(venue.clone());
            continue;
        }
        // Cloned first: reading the account borrows the adapter mutably.
        let catalogue = adapter.declaration().instruments().clone();
        // The instant is this node's. An adapter that reads its own clock stamps its own.
        let answered = contain(adapter.account(crate::clock::now_ns())).await;
        let Ok(Ok(snapshot)) = answered else {
            if answered.is_err() {
                faulted.insert(venue.clone());
            }
            withdrawn.push(venue.clone());
            continue;
        };
        match resolve(venue, &catalogue, &snapshot.positions) {
            Some(venue_held) => {
                oldest = oldest.min(snapshot.observed_at_ns);
                held.extend(venue_held);
                margins.push((venue.clone(), snapshot.margin));
            }
            None => withdrawn.push(venue.clone()),
        }
    }

    let observed_at_ns = if oldest == i128::MAX { 0 } else { oldest };
    Holdings { held, withdrawn, observed_at_ns, margins }
}

/// Maps a venue's positions to platform identities. All or nothing: a partly resolved venue
/// would make the total look complete.
fn resolve(
    venue: &str,
    catalogue: &bijective_venue::profile::InstrumentCatalogue,
    positions: &[bijective_venue::contract::PositionSnapshot],
) -> Option<Vec<HeldPosition>> {
    let mut venue_held = Vec::with_capacity(positions.len());
    for position in positions {
        let id = catalogue.identity_of(&position.instrument).ok()?;
        venue_held.push(HeldPosition {
            venue: venue.to_string(),
            instrument: id.clone(),
            quantity_scaled: position.quantity_scaled,
            entry_price_scaled: position.entry_price_scaled,
        });
    }
    Some(venue_held)
}
