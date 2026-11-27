// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! How far a live session was sent, and how the next one finds out.
//! A run ledger entry is written at run end, which a killed supervisor never reaches.

use std::collections::BTreeMap;

use bijective_event_store::digest::Digest;
use bijective_event_store::projection::Projection;
use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

/// Version 2 names the market stream a mark is for, so a session reading several keeps one mark
/// per stream. Version 1 marks, from sessions that read one stream, are still read.
pub const VERSION: u16 = 2;

/// Events between marks. A crash overstates the next gap by at most this many, the safe direction,
/// so this is tuned for record count, not precision.
pub const MARK_EVERY: u64 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMark {
    pub strategy: Digest,
    /// The market stream `sent_through` is a position on. `None` on a version 1 mark, which did
    /// not say.
    pub market_stream: Option<String>,
    pub sent_through: u64,
    /// When the supervisor wrote it. Never the strategy's own clock.
    pub at_ns: i128,
}

pub fn encode(mark: &SessionMark) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::StrategySessionMark, VERSION);
    w.bytes(&mark.strategy.0)?;
    match &mark.market_stream {
        Some(stream) => w.u8(1).str(stream)?,
        None => w.u8(0),
    };
    w.u64(mark.sent_through);
    w.i128(mark.at_ns);
    Ok(w.finish())
}

/// Decode a payload. An unknown class or a newer version is refused, never skipped.
pub fn decode(payload: &[u8]) -> Result<SessionMark, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::StrategySessionMark {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let raw = r.bytes()?;
    let mut strategy = [0u8; 32];
    if raw.len() != strategy.len() {
        return Err(DecodeError::InvalidValue { field: "strategy", found: raw.len() as u64 });
    }
    strategy.copy_from_slice(raw);
    let stream = match (envelope.version, if envelope.version >= 2 { r.u8()? } else { 0 }) {
        (1, _) | (_, 0) => None,
        (_, 1) => Some(r.str()?.to_string()),
        (_, other) => {
            return Err(DecodeError::InvalidValue { field: "stream", found: u64::from(other) });
        }
    };

    Ok(SessionMark {
        strategy: Digest(strategy),
        market_stream: stream,
        sent_through: r.u64()?,
        at_ns: r.i128()?,
    })
}

/// Per strategy and stream, not one number: two strategies run side by side and would see each
/// other's progress, and positions on two streams are not comparable.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SentThrough {
    furthest: BTreeMap<(String, Option<String>), u64>,
}

impl SentThrough {
    /// How far `strategy` was sent through `stream`. A session that reads only that stream also
    /// takes a version 1 mark, which was written by a session of one stream. A session of several
    /// does not, since that mark may be a position on another stream. None means not recorded:
    /// read it as everything before the resume point, not as nothing missed.
    #[must_use]
    pub fn of(&self, strategy: &Digest, stream: &str, only_stream: bool) -> Option<u64> {
        let named = self.furthest.get(&(strategy.to_hex(), Some(stream.to_string()))).copied();
        let unnamed = self.furthest.get(&(strategy.to_hex(), None)).copied();
        match (named, only_stream) {
            (Some(through), _) => Some(through),
            (None, true) => unnamed,
            (None, false) => None,
        }
    }
}

impl Projection for SentThrough {
    type Event = SessionMark;
    type Error = DecodeError;

    fn decode(payload: &[u8]) -> Result<SessionMark, DecodeError> {
        decode(payload)
    }

    fn apply(&mut self, mark: &SessionMark) -> Result<(), DecodeError> {
        // Highest seen, not last applied: a figure that moved backwards would understate a gap, which section 2.6 refuses.
        let key = (mark.strategy.to_hex(), mark.market_stream.clone());
        let held = self.furthest.entry(key).or_insert(mark.sent_through);
        *held = (*held).max(mark.sent_through);
        Ok(())
    }
}
