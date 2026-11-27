// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The frame journal: every input the data engine acts on, written before it acts.
//!
//! An entry is a frame exactly as the venue sent it with its arrival time, or one of the events
//! that change how later frames are read (books dropped, a session starting or ending). Replaying
//! the journal through the same feed reproduces every record, top and archive row, so the journal
//! is the durable record and everything downstream is a projection of it.

use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

pub const VERSION: u16 = 1;

const FRAME: u8 = 0;
const CONTINUITY_LOST: u8 = 1;
const SESSION_STARTED: u8 = 2;
const SESSION_ENDED: u8 = 3;

/// One writer per stream, so one per venue.
#[must_use]
pub fn stream(venue: &str) -> String {
    format!("frames-{venue}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEntry {
    /// The bytes are opaque here. Only the venue's feed reads them.
    Frame {
        received_at_ns: i128,
        frame: Vec<u8>,
    },
    /// Books were dropped at this instant. A replay drops its books at the same entry.
    ContinuityLost {
        at_ns: i128,
    },
    SessionStarted {
        at_ns: i128,
    },
    SessionEnded {
        at_ns: i128,
    },
}

pub fn encode(entry: &JournalEntry) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::MarketJournalEntry, VERSION);
    match entry {
        JournalEntry::Frame { received_at_ns, frame } => {
            w.u8(FRAME).i128(*received_at_ns).bytes(frame)?;
        }
        JournalEntry::ContinuityLost { at_ns } => {
            w.u8(CONTINUITY_LOST).i128(*at_ns);
        }
        JournalEntry::SessionStarted { at_ns } => {
            w.u8(SESSION_STARTED).i128(*at_ns);
        }
        JournalEntry::SessionEnded { at_ns } => {
            w.u8(SESSION_ENDED).i128(*at_ns);
        }
    }
    Ok(w.finish())
}

pub fn decode(payload: &[u8]) -> Result<JournalEntry, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.class != ClassId::MarketJournalEntry {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    if envelope.version != VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    let entry = match r.u8()? {
        FRAME => {
            let received_at_ns = r.i128()?;
            JournalEntry::Frame { received_at_ns, frame: r.bytes()?.to_vec() }
        }
        CONTINUITY_LOST => JournalEntry::ContinuityLost { at_ns: r.i128()? },
        SESSION_STARTED => JournalEntry::SessionStarted { at_ns: r.i128()? },
        SESSION_ENDED => JournalEntry::SessionEnded { at_ns: r.i128()? },
        found => return Err(DecodeError::InvalidValue { field: "entry", found: u64::from(found) }),
    };
    r.finish()?;
    Ok(entry)
}

#[cfg(test)]
#[path = "../tests/unit/journal_tests.rs"]
mod tests;
