// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! C1 decision-capture core.
//! Records the market as it looked when a strategy decided, so execution is measured against what was achievable.

use std::collections::{HashMap, VecDeque};

/// Fidelity of a timestamp, kept so a second-grade value is not read as nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Nanosecond,
    Microsecond,
    Millisecond,
    Second,
}

/// A point in time in nanoseconds. `resolution` states how much of that precision is real.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instant {
    pub value_ns: i128,
    pub resolution: Resolution,
}

impl Instant {
    pub fn new(value_ns: i128, resolution: Resolution) -> Self {
        Self { value_ns, resolution }
    }
}

/// Top of book at a single instant, for one instrument. Integer ticks and lots, never floats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookSnapshot {
    pub instrument: String,
    pub best_bid_ticks: i64,
    pub best_bid_qty_lots: i64,
    pub best_ask_ticks: i64,
    pub best_ask_qty_lots: i64,
    pub observed_at: Instant,
}

/// A market-data update delivered to the buffer. An update is applied, a snapshot is stamped.
#[derive(Clone, Debug)]
pub struct BookUpdate {
    pub instrument: String,
    pub best_bid_ticks: i64,
    pub best_bid_qty_lots: i64,
    pub best_ask_ticks: i64,
    pub best_ask_qty_lots: i64,
    pub observed_at: Instant,
}

/// What the strategy decided, when, and what the market looked like then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionRecord {
    pub correlation_id: String,
    pub instrument: String,
    pub decided_at: Instant,
    pub book: BookSnapshot,
    /// `decided_at - book.observed_at` in nanoseconds, never negative.
    pub capture_lag_ns: i128,
}

/// Why a capture could not produce a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    NoSnapshotForInstrument,
    SnapshotStale {
        age_ns: i128,
        bound_ns: i128,
    },
    /// The held snapshot is newer than the decision: a clock inconsistency, refused rather than a negative lag.
    SnapshotAfterDecision,
    /// The snapshot held at the decision has left the history, by age or by its cap: the decision
    /// was judged later than the history reaches.
    SnapshotEvicted,
    /// The buffer is degraded and the instrument was never admitted (see `BufferState`).
    BufferDegraded,
}

/// The health of the buffer.
/// `Degraded` is surfaced for missing-event detection to alarm on. Silently dropping data is the failure to prevent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferState {
    Healthy,
    Degraded,
}

/// Snapshots kept per instrument. The decision's book is older than the newest one by the time a
/// proposal is judged, so a history is kept. The cap bounds it whatever the feed's rate.
pub const HISTORY_PER_INSTRUMENT: usize = 4096;

/// A bounded, point-in-time top-of-book buffer: per instrument, the snapshots of the last
/// `max_age_ns`, oldest first, at most [`HISTORY_PER_INSTRUMENT`] of them.
/// Bounded by instrument count, entry age and history length, so memory depends on the caps and
/// not on the feed.
pub struct SnapshotBuffer {
    max_instruments: usize,
    max_age_ns: i128,
    entries: HashMap<String, History>,
    state: BufferState,
    /// Admissions refused because the instrument cap was reached.
    degraded_events: u64,
}

/// One instrument's snapshots, oldest first, and the newest one that has left them.
#[derive(Default)]
struct History {
    snapshots: VecDeque<BookSnapshot>,
    trimmed_through_ns: Option<i128>,
}

impl History {
    fn trim(&mut self) {
        if let Some(gone) = self.snapshots.pop_front() {
            self.trimmed_through_ns = Some(gone.observed_at.value_ns);
        }
    }
}

impl SnapshotBuffer {
    /// Both bounds must be positive. Zero is rejected at construction.
    pub fn new(max_instruments: usize, max_age_ns: i128) -> Self {
        assert!(max_instruments > 0, "max_instruments must be positive");
        assert!(max_age_ns > 0, "max_age_ns must be positive");
        Self {
            max_instruments,
            max_age_ns,
            entries: HashMap::new(),
            state: BufferState::Healthy,
            degraded_events: 0,
        }
    }

    pub fn state(&self) -> BufferState {
        self.state
    }

    /// Number of instruments currently held. Never exceeds `max_instruments`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn degraded_events(&self) -> u64 {
        self.degraded_events
    }

    /// Apply a market-data update at wall-clock `now_ns`. Updates arrive in the order they were
    /// observed. One observed before the newest held is not kept, so the history stays ordered.
    /// A new instrument is admitted only if there is room after age eviction. Otherwise it is refused and the buffer degraded.
    pub fn on_market_data(&mut self, update: BookUpdate, now_ns: i128) {
        self.evict_expired(now_ns);

        let snapshot = BookSnapshot {
            instrument: update.instrument.clone(),
            best_bid_ticks: update.best_bid_ticks,
            best_bid_qty_lots: update.best_bid_qty_lots,
            best_ask_ticks: update.best_ask_ticks,
            best_ask_qty_lots: update.best_ask_qty_lots,
            observed_at: update.observed_at,
        };

        // Only a full-and-new instrument is refused. That keeps the bound hard.
        let already_held = self.entries.contains_key(&update.instrument);
        if already_held || self.entries.len() < self.max_instruments {
            let history = self.entries.entry(update.instrument).or_default();
            let newest = history.snapshots.back().map(|held| held.observed_at.value_ns);
            if newest.is_some_and(|newest| snapshot.observed_at.value_ns < newest) {
                return;
            }
            if history.snapshots.len() == HISTORY_PER_INSTRUMENT {
                history.trim();
            }
            history.snapshots.push_back(snapshot);
        } else {
            // Refuse and degrade rather than grow. Missing-event detection alarms on the state.
            self.state = BufferState::Degraded;
            self.degraded_events += 1;
        }
    }

    /// Stamp the newest snapshot for `instrument` observed at or before a decision.
    /// `decided_at` is the decision time, never the order submission time, which would inflate the benchmark lag.
    pub fn capture(
        &self,
        instrument: &str,
        correlation_id: &str,
        decided_at: Instant,
        stale_bound_ns: i128,
    ) -> Result<DecisionRecord, CaptureError> {
        let history = match self.entries.get(instrument) {
            Some(history) => history,
            // Absent while the buffer is refusing admissions: research integrity must tell this from an instrument with no subscription.
            None if self.state == BufferState::Degraded => {
                return Err(CaptureError::BufferDegraded);
            }
            None => return Err(CaptureError::NoSnapshotForInstrument),
        };

        // Only what had been observed when the strategy decided: a later book would be look-ahead.
        let decided = decided_at.value_ns;
        let held = history.snapshots.iter().rev().find(|s| s.observed_at.value_ns <= decided);
        let Some(snapshot) = held else {
            if history.trimmed_through_ns.is_some_and(|gone| gone <= decided) {
                return Err(CaptureError::SnapshotEvicted);
            }
            return Err(CaptureError::SnapshotAfterDecision);
        };
        let lag = decided_at.value_ns - snapshot.observed_at.value_ns;
        if lag > stale_bound_ns {
            return Err(CaptureError::SnapshotStale { age_ns: lag, bound_ns: stale_bound_ns });
        }

        Ok(DecisionRecord {
            correlation_id: correlation_id.to_string(),
            instrument: instrument.to_string(),
            decided_at,
            book: snapshot.clone(),
            capture_lag_ns: lag,
        })
    }

    fn evict_expired(&mut self, now_ns: i128) {
        let max_age = self.max_age_ns;
        for history in self.entries.values_mut() {
            let expired = |s: &BookSnapshot| now_ns - s.observed_at.value_ns > max_age;
            while history.snapshots.front().is_some_and(expired) {
                history.trim();
            }
        }
        self.entries.retain(|_, history| !history.snapshots.is_empty());
    }
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

impl core::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CaptureError::NoSnapshotForInstrument => {
                write!(f, "no snapshot has arrived for this instrument")
            }
            CaptureError::SnapshotStale { age_ns, bound_ns } => {
                write!(f, "the snapshot is {age_ns}ns old against a bound of {bound_ns}ns")
            }
            CaptureError::SnapshotAfterDecision => {
                write!(f, "the snapshot is newer than the decision it would explain")
            }
            CaptureError::SnapshotEvicted => {
                write!(f, "the snapshot held at the decision has left the history")
            }
            CaptureError::BufferDegraded => write!(f, "the capture buffer is degraded"),
        }
    }
}
