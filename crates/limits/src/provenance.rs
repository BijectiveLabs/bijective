// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Bounded provenance recording, run after the safe state is driven.

use super::*;

impl core::fmt::Display for RecordOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RecordOutcome::Written => write!(f, "written"),
            RecordOutcome::Retained { pending } => write!(f, "retained, {pending} pending"),
            // Running total, not this drop: a climbing total is what an operator acts on.
            RecordOutcome::Dropped { dropped_total } => {
                write!(f, "dropped, {dropped_total} in total since the run began")
            }
        }
    }
}

/// Assembled at decision time, not reconstructed later from sources that can disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub at_ns: i128,
    pub reason: SafeStateReason,
    pub state_driven: SafeState,
    /// Lets an operator say whether the triggering figure was live.
    pub freshness: Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkUnavailable;

/// A port. Production writes to the core event store.
/// Append only: an editable record could retroactively justify or deny a halt.
pub trait ProvenanceSink {
    fn append(&mut self, record: ProvenanceRecord) -> Result<(), SinkUnavailable>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    Written,
    Retained {
        pending: usize,
    },
    /// The count is the evidence that evidence is missing. Without it the audit pack looks unbroken.
    Dropped {
        dropped_total: u64,
    },
}

/// Safety is never gated on recording: the safe state is driven first. On overflow the newest
/// record is dropped, not the oldest, because provenance explains how an incident began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorder {
    pub(crate) pending: Vec<ProvenanceRecord>,
    pub(crate) capacity: usize,
    pub(crate) dropped: u64,
}

impl Recorder {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { pending: Vec::new(), capacity, dropped: 0 }
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Non-zero means the audit trail has a gap, stated rather than inferred.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn record(
        &mut self,
        sink: &mut impl ProvenanceSink,
        record: ProvenanceRecord,
    ) -> RecordOutcome {
        match sink.append(record.clone()) {
            Ok(()) => RecordOutcome::Written,
            Err(SinkUnavailable) => {
                if self.pending.len() < self.capacity {
                    self.pending.push(record);
                    RecordOutcome::Retained { pending: self.pending.len() }
                } else {
                    self.dropped += 1;
                    RecordOutcome::Dropped { dropped_total: self.dropped }
                }
            }
        }
    }

    /// Stops at the first failure and keeps everything unwritten, so a flapping store loses nothing.
    pub fn flush(&mut self, sink: &mut impl ProvenanceSink) -> Result<usize, SinkUnavailable> {
        let mut written = 0;
        while let Some(record) = self.pending.first() {
            match sink.append(record.clone()) {
                Ok(()) => {
                    self.pending.remove(0);
                    written += 1;
                }
                Err(SinkUnavailable) => return Err(SinkUnavailable),
            }
        }
        Ok(written)
    }
}
