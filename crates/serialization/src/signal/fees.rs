// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a venue charges to cross its spread.
//! A declaration, not a call: carries its source and read time. venue assurance. In the core per the platform architecture.

use std::path::Path;

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_event_store::store::{StoreError, read_range};

pub const VERSION: u16 = 1;

/// The stream a venue's fees are declared on.
pub const FEES: &str = "venue-fees";

/// What one venue charges, in basis points in the platform's fixed scale.
/// Positive is a cost. `maker` may be negative where a venue pays for resting liquidity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fees {
    pub venue: String,
    pub taker: i128,
    pub maker: i128,
    /// Where the figures came from, in a form an operator can verify.
    pub source: String,
    /// When they were read from that source. Never when they were written here.
    pub read_at_ns: i128,
}

#[derive(Debug)]
pub enum FeesError {
    Encode(EncodeError),
    Decode(DecodeError),
    Store(StoreError),
    /// A declaration with nothing anybody could check.
    NoSource,
}

impl core::fmt::Display for FeesError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FeesError::Encode(e) => write!(f, "the fees could not be encoded: {e:?}"),
            FeesError::Decode(e) => write!(f, "the fees could not be read: {e:?}"),
            FeesError::Store(e) => write!(f, "store: {e:?}"),
            FeesError::NoSource => {
                write!(f, "a fee with no source cannot be verified")
            }
        }
    }
}

impl std::error::Error for FeesError {}

pub fn encode(fees: &Fees) -> Result<Vec<u8>, FeesError> {
    if fees.source.trim().is_empty() {
        return Err(FeesError::NoSource);
    }
    let mut w = Writer::new(ClassId::VenueFeesDeclared, VERSION);
    w.str(&fees.venue).map_err(FeesError::Encode)?;
    w.i128(fees.taker);
    w.i128(fees.maker);
    w.str(&fees.source).map_err(FeesError::Encode)?;
    w.i128(fees.read_at_ns);
    Ok(w.finish())
}

pub fn decode(payload: &[u8]) -> Result<Fees, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::VenueFeesDeclared {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    Ok(Fees {
        venue: r.str()?.to_string(),
        taker: r.i128()?,
        maker: r.i128()?,
        source: r.str()?.to_string(),
        read_at_ns: r.i128()?,
    })
}

/// The declaration that stands for a venue, or none.
/// The latest wins and nothing is deleted, so a measurement made under an earlier one can still be read.
pub fn standing(root: &Path, venue: &str) -> Result<Option<Fees>, FeesError> {
    let entries = read_range(root, FEES, 0, u64::MAX).map_err(FeesError::Store)?;

    let mut found = None;
    for entry in entries {
        // Refused, never skipped: the unreadable entry may be the standing declaration.
        let fees = decode(&entry.payload).map_err(FeesError::Decode)?;
        if fees.venue == venue {
            found = Some(fees);
        }
    }
    Ok(found)
}

#[cfg(test)]
#[path = "../../tests/unit/signal/fees_tests.rs"]
mod tests;
