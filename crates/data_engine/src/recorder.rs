// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The recorder: one path from what arrived to what is stored, shared by live ingest and replay.
//!
//! Order is the contract. A frame is journaled, and synced, before anything parsed from it is
//! stored or forwarded, so nothing downstream ever holds data the journal does not. Records go to
//! the archive, facts to the facts stream, the top of book to the market stream, and every event
//! that makes the record incomplete becomes a gap row. Replay repeats every step. Syncs are per
//! batch (decision 0019): facts and tops are held until `commit` has synced their frames, because
//! a reader sees an entry once it is written, not once it is synced.

use bijective_archive::{
    ArchiveConfig, ArchiveCounts, ArchiveError, ArchiveWriter, GapKind, GapRow, OpenReport,
};
use bijective_event_store::store::{StoreError, StreamWriter};
use bijective_serialization::codec::writer::EncodeError;
use bijective_serialization::signal::facts::{self, FactError, FactUpdate};
use bijective_venue::contract::{
    BookTop, FeedError, MarketFeed, MarketRecord, Offered, VenueSymbol,
};
use bijective_venue::profile::InstrumentCatalogue;

use crate::journal::{self, JournalEntry};
use crate::{BookUpdate, encode};

#[derive(Debug)]
pub enum RecorderError {
    /// The journal refused an entry. Nothing after it may be acted on.
    Journal(StoreError),
    Market(StoreError),
    Facts(StoreError),
    Encode(EncodeError),
    Archive(ArchiveError),
    /// Symbols are not unique across venues, so an undeclared one is refused, not written.
    NotInCatalogue(VenueSymbol),
    /// A record or refusal with no journaled frame before it: it could be traced to no bytes.
    NoFrame,
}

impl core::fmt::Display for RecorderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Journal(e) => write!(f, "journal append refused: {e}"),
            Self::Market(e) => write!(f, "market append refused: {e}"),
            Self::Facts(e) => write!(f, "facts append refused: {e}"),
            Self::Encode(e) => write!(f, "encode refused: {e:?}"),
            Self::Archive(e) => write!(f, "archive refused: {e}"),
            Self::NotInCatalogue(s) => write!(f, "{s} is not in this venue's declared catalogue"),
            Self::NoFrame => write!(f, "a record arrived before any frame was journaled"),
        }
    }
}

impl std::error::Error for RecorderError {}

impl From<ArchiveError> for RecorderError {
    fn from(e: ArchiveError) -> Self {
        Self::Archive(e)
    }
}

/// Counts an operator reads in the readiness detail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecorderCounts {
    pub frames: u64,
    pub tops: u64,
    pub records: u64,
    /// Facts written to the facts stream.
    pub facts: u64,
    /// Facts on a symbol the catalogue does not declare: kept in the journal and the archive, not
    /// sent to strategies, because no platform identity names them.
    pub undeclared_facts: u64,
    pub gaps: u64,
}

/// Most events one commit carries. A live ingest drains what is waiting up to this many, so an idle
/// stream commits every frame and a burst shares one flush across the batch.
pub const BATCH_LIMIT: usize = 256;

/// Which derived stream a held entry is written to at commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Derived {
    Market,
    Facts,
}

/// Where the recorder writes what strategies read: the top of book and the other facts.
pub struct Outputs<'a> {
    pub market: &'a mut StreamWriter,
    pub facts: &'a mut StreamWriter,
}

/// The journal entry the current frame was written at, and its arrival.
#[derive(Debug, Clone, Copy)]
struct CurrentFrame {
    seq: u64,
    received_at_ns: i128,
    records: u32,
}

pub struct Recorder<'a> {
    journal: StreamWriter,
    outputs: Outputs<'a>,
    catalogue: &'a InstrumentCatalogue,
    archive: Option<ArchiveWriter>,
    current: Option<CurrentFrame>,
    counts: RecorderCounts,
    /// Derived entries written at the next commit, after the journal they come from is durable.
    held: Vec<(Derived, Vec<u8>)>,
}

impl<'a> Recorder<'a> {
    /// Opens the archive, when one is configured, before anything is journaled.
    pub fn new(
        journal: StreamWriter,
        outputs: Outputs<'a>,
        catalogue: &'a InstrumentCatalogue,
        archive: Option<(ArchiveConfig, i128)>,
    ) -> Result<(Self, Option<OpenReport>), RecorderError> {
        let (archive, report) = match archive {
            Some((config, at_ns)) => {
                let (writer, report) = ArchiveWriter::open(config, at_ns)?;
                (Some(writer), Some(report))
            }
            None => (None, None),
        };
        let recorder = Self {
            journal,
            outputs,
            catalogue,
            archive,
            current: None,
            counts: RecorderCounts::default(),
            held: Vec::new(),
        };
        Ok((recorder, report))
    }

    pub fn counts(&self) -> RecorderCounts {
        self.counts
    }

    pub fn archive_counts(&self) -> Option<ArchiveCounts> {
        self.archive.as_ref().map(ArchiveWriter::counts)
    }

    fn journal(&mut self, entry: &JournalEntry) -> Result<u64, RecorderError> {
        let payload = journal::encode(entry).map_err(RecorderError::Encode)?;
        self.journal.append_unsynced(&payload).map_err(RecorderError::Journal)
    }

    /// Makes the batch durable: the journal first, then what was derived from it, then that.
    /// Postcondition: every entry passed in since the last commit is on disk, and no derived entry
    /// was written before its frame was durable. Cost: one sync of the journal and at most one of
    /// each derived stream, however many frames the batch held.
    pub fn commit(&mut self) -> Result<(), RecorderError> {
        self.journal.sync().map_err(RecorderError::Journal)?;
        let (mut market, mut facts) = (false, false);
        for (stream, payload) in self.held.drain(..) {
            match stream {
                Derived::Market => {
                    market = true;
                    self.outputs.market.append_unsynced(&payload).map_err(RecorderError::Market)?;
                }
                Derived::Facts => {
                    facts = true;
                    self.outputs.facts.append_unsynced(&payload).map_err(RecorderError::Facts)?;
                }
            }
        }
        if market {
            self.outputs.market.sync().map_err(RecorderError::Market)?;
        }
        if facts {
            self.outputs.facts.sync().map_err(RecorderError::Facts)?;
        }
        Ok(())
    }

    fn gap(
        &mut self,
        seq: u64,
        at_ns: i128,
        kind: GapKind,
        detail: String,
    ) -> Result<(), RecorderError> {
        self.counts.gaps += 1;
        match self.archive.as_mut() {
            Some(archive) => Ok(archive.mark_gap(GapRow::new(seq, at_ns, kind, detail)?)?),
            None => Ok(()),
        }
    }

    pub fn session_started(&mut self, at_ns: i128) -> Result<(), RecorderError> {
        let seq = self.journal(&JournalEntry::SessionStarted { at_ns })?;
        self.gap(seq, at_ns, GapKind::SessionStarted, String::new())
    }

    pub fn session_ended(&mut self, at_ns: i128) -> Result<(), RecorderError> {
        let seq = self.journal(&JournalEntry::SessionEnded { at_ns })?;
        self.gap(seq, at_ns, GapKind::SessionEnded, String::new())
    }

    /// Postcondition: the frame is written, and durable at the next `commit`.
    pub fn frame(&mut self, received_at_ns: i128, frame: &[u8]) -> Result<(), RecorderError> {
        let seq = self.journal(&JournalEntry::Frame { received_at_ns, frame: frame.to_vec() })?;
        self.current = Some(CurrentFrame { seq, received_at_ns, records: 0 });
        self.counts.frames += 1;
        Ok(())
    }

    pub fn continuity_lost(&mut self, at_ns: i128) -> Result<(), RecorderError> {
        let seq = self.journal(&JournalEntry::ContinuityLost { at_ns })?;
        self.gap(seq, at_ns, GapKind::ContinuityLost, String::new())
    }

    /// Precondition: the record's frame was the last one passed to `frame`.
    pub fn record(&mut self, record: &MarketRecord) -> Result<(), RecorderError> {
        let current = self.current.ok_or(RecorderError::NoFrame)?;
        if let Some(archive) = self.archive.as_mut() {
            archive.append_record(record, current.seq, current.records)?;
        }
        self.current = Some(CurrentFrame { records: current.records + 1, ..current });
        self.counts.records += 1;
        self.forward_fact(record)
    }

    /// Writes a fact for strategies, named by the platform's identity.
    fn forward_fact(&mut self, record: &MarketRecord) -> Result<(), RecorderError> {
        if !facts::is_fact(&record.fact) {
            return Ok(());
        }
        let Ok(instrument) = self.catalogue.identity_of(&record.instrument) else {
            self.counts.undeclared_facts += 1;
            return Ok(());
        };
        let update = FactUpdate {
            instrument: instrument.clone(),
            exchange_time: record.exchange_time,
            received_at_ns: record.received_at_ns,
            fact: record.fact.clone(),
        };
        let payload = match facts::encode(&update) {
            Ok(payload) => payload,
            Err(FactError::Encode(e)) => return Err(RecorderError::Encode(e)),
            Err(FactError::NotAFact) => return Ok(()),
        };
        self.held.push((Derived::Facts, payload));
        self.counts.facts += 1;
        Ok(())
    }

    pub fn top(&mut self, top: &BookTop) -> Result<(), RecorderError> {
        // The top came from the frame being handled, so it arrived when that frame did.
        let received_at_ns = self.current.map(|c| c.received_at_ns);
        let update = from_top(top, self.catalogue, received_at_ns)?;
        let payload = encode(&update).map_err(RecorderError::Encode)?;
        self.held.push((Derived::Market, payload));
        self.counts.tops += 1;
        Ok(())
    }

    /// The refused frame is already in the journal. The gap says the record lacks what it held.
    pub fn refused(&mut self, error: &FeedError) -> Result<(), RecorderError> {
        let current = self.current.ok_or(RecorderError::NoFrame)?;
        self.gap(current.seq, current.received_at_ns, GapKind::FrameRefused, error.to_string())
    }

    /// Records first, then the top, as a live stream forwards them.
    pub fn offered(&mut self, offered: &Offered) -> Result<(), RecorderError> {
        for record in &offered.records {
            self.record(record)?;
        }
        match &offered.top {
            Some(top) => self.top(top),
            None => Ok(()),
        }
    }

    /// Replays one journal entry through `feed`, journaling it again as it was.
    pub fn replay(
        &mut self,
        entry: &JournalEntry,
        feed: &mut dyn MarketFeed,
    ) -> Result<(), RecorderError> {
        match entry {
            JournalEntry::Frame { received_at_ns, frame } => {
                self.frame(*received_at_ns, frame)?;
                match feed.offer(frame, *received_at_ns) {
                    Ok(offered) => self.offered(&offered),
                    Err(e) => self.refused(&e),
                }
            }
            JournalEntry::ContinuityLost { at_ns } => {
                feed.reset();
                self.continuity_lost(*at_ns)
            }
            JournalEntry::SessionStarted { at_ns } => self.session_started(*at_ns),
            JournalEntry::SessionEnded { at_ns } => self.session_ended(*at_ns),
        }
    }

    /// Commits what is held, then seals the archive.
    pub fn finish(mut self, at_ns: i128) -> Result<Option<ArchiveCounts>, RecorderError> {
        self.commit()?;
        match self.archive {
            Some(archive) => Ok(Some(archive.close(at_ns)?)),
            None => Ok(None),
        }
    }
}

/// Refused, not written under the venue symbol: symbols are not unique across venues, so a shared
/// stream could let one venue's quote price another's position.
fn from_top(
    top: &BookTop,
    catalogue: &InstrumentCatalogue,
    received_at_ns: Option<i128>,
) -> Result<BookUpdate, RecorderError> {
    let instrument = catalogue
        .identity_of(&top.instrument)
        .map_err(|_| RecorderError::NotInCatalogue(top.instrument.clone()))?
        .clone();

    Ok(BookUpdate {
        instrument,
        seq: top.sequence,
        venue_ts_ms: top.venue_ts_ms,
        received_at_ns,
        bid_price: top.bid_price,
        bid_quantity: top.bid_quantity,
        ask_price: top.ask_price,
        ask_quantity: top.ask_quantity,
        bid_depth: top.bid_depth,
        ask_depth: top.ask_depth,
    })
}

#[cfg(test)]
#[path = "../tests/unit/recorder_tests.rs"]
mod tests;
