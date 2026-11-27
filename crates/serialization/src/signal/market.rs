// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The market book-update layout, at version 4.
//! Top of book rather than the full stage sequence: execution quality attribution needs it aligned with decisions, not reproduced.

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_model::{InstrumentId, Sequence};

/// Version 4 adds the platform's receive time. Version 3 lets a venue say it numbers nothing.
/// Version 2 carried the platform's identity rather than the venue's symbol.
/// Version 3 entries are still read, with no receive time. Version-1 and version-2 entries are
/// refused on replay: a bare venue symbol cannot be resolved to an identity.
pub const VERSION: u16 = 4;

/// The oldest version still read. Older stores are read as they are, never rewritten.
const OLDEST_READ: u16 = 3;

/// The stream one venue's book updates are written to.
/// Defined here, not at either end, so the writer and reader share one convention.
#[must_use]
pub fn stream(venue: &str) -> String {
    // A hyphen, not a dot: the store admits only alphanumerics, `-` and `_` in a stream name.
    format!("market-{venue}")
}

/// The venue a market stream belongs to, or nothing where the name is not one of ours.
/// The inverse of `stream`, so the naming convention lives in one place.
#[must_use]
pub fn venue_of(stream: &str) -> Option<&str> {
    stream.strip_prefix("market-").filter(|rest| !rest.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookUpdate {
    /// The platform's identity, not the venue's symbol. Resolved by the Data Engine through the declared catalogue.
    pub instrument: InstrumentId,
    /// What the venue numbered it, where it numbers at all.
    pub seq: Sequence,
    pub venue_ts_ms: u64,
    /// The platform's clock when the frame carrying this top was read from the socket, in
    /// nanoseconds since the Unix epoch. One clock for every venue, so it orders tops across
    /// venues as this node could know them. `None` on a top recorded before version 4.
    pub received_at_ns: Option<i128>,
    pub bid_price: i128,
    pub bid_quantity: i128,
    pub ask_price: i128,
    pub ask_quantity: i128,
    pub bid_depth: u32,
    pub ask_depth: u32,
}

pub fn encode(u: &BookUpdate) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::MarketBookUpdate, VERSION);
    w.str(u.instrument.as_str())?;
    match u.seq {
        Sequence::Numbered(n) => w.u8(1).u64(n),
        Sequence::NotNumbered => w.u8(0).u64(0),
    };
    w.u64(u.venue_ts_ms)
        .i128(u.bid_price)
        .i128(u.bid_quantity)
        .i128(u.ask_price)
        .i128(u.ask_quantity)
        .u32(u.bid_depth)
        .u32(u.ask_depth);
    match u.received_at_ns {
        Some(at) => w.u8(1).i128(at),
        None => w.u8(0).i128(0),
    };
    Ok(w.finish())
}

pub fn decode(payload: &[u8]) -> Result<BookUpdate, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.class != ClassId::MarketBookUpdate {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    if !(OLDEST_READ..=VERSION).contains(&envelope.version) {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }

    let instrument = InstrumentId::new(r.str()?);
    let mut out = BookUpdate {
        instrument,
        seq: {
            let numbered = r.u8()?;
            let value = r.u64()?;
            match numbered {
                0 => Sequence::NotNumbered,
                1 => Sequence::Numbered(value),
                found => {
                    return Err(DecodeError::InvalidValue {
                        field: "seq",
                        found: u64::from(found),
                    });
                }
            }
        },
        venue_ts_ms: r.u64()?,
        bid_price: r.i128()?,
        bid_quantity: r.i128()?,
        ask_price: r.i128()?,
        ask_quantity: r.i128()?,
        bid_depth: r.u32()?,
        ask_depth: r.u32()?,
        received_at_ns: None,
    };
    if envelope.version >= 4 {
        let stamped = r.u8()?;
        let at = r.i128()?;
        out.received_at_ns = match stamped {
            0 => None,
            1 => Some(at),
            found => {
                return Err(DecodeError::InvalidValue {
                    field: "received_at_ns",
                    found: u64::from(found),
                });
            }
        };
    }
    r.finish()?;
    Ok(out)
}

#[cfg(test)]
#[path = "../../tests/unit/signal/market_tests.rs"]
mod stream_tests;
