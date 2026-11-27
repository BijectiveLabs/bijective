// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the link to one venue did, and when this platform saw it.
//! The instant is the platform's, not the venue's. Untracked venue instants stay `Moment::NotTracked`.

use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_venue::contract::{ConnectionState, DegradedReason, Moment};

pub const VERSION: u16 = 1;

const STATE_DISCONNECTED: u8 = 0;
const STATE_CONNECTING: u8 = 1;
const STATE_CONNECTED: u8 = 2;
const STATE_DEGRADED: u8 = 3;

const TROUBLE_BACKLOGGED: u8 = 0;
const TROUBLE_STALE: u8 = 1;
const TROUBLE_INCOMPLETE: u8 = 2;

/// One writer per stream, so one per venue.
#[must_use]
pub fn stream(venue: &str) -> String {
    format!("link-{venue}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTransition {
    pub state: ConnectionState,
    pub at_ns: i128,
}

pub fn encode(t: &LinkTransition) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::LinkTransition, VERSION);

    match &t.state {
        ConnectionState::Disconnected => {
            w.u8(STATE_DISCONNECTED).u8(0).usize_pair(0, 0);
        }
        ConnectionState::Connecting => {
            w.u8(STATE_CONNECTING).u8(0).usize_pair(0, 0);
        }
        ConnectionState::Connected { .. } => {
            w.u8(STATE_CONNECTED).u8(0).usize_pair(0, 0);
        }
        ConnectionState::Degraded { reason, .. } => {
            w.u8(STATE_DEGRADED);
            match reason {
                DegradedReason::Backlogged { queued } => {
                    w.u8(TROUBLE_BACKLOGGED).usize_pair(*queued, 0);
                }
                DegradedReason::Stale { .. } => {
                    w.u8(TROUBLE_STALE).usize_pair(0, 0);
                }
                DegradedReason::SubscriptionsIncomplete { attached, requested } => {
                    w.u8(TROUBLE_INCOMPLETE).usize_pair(*attached, *requested);
                }
            }
        }
    }

    w.i128(t.at_ns);
    Ok(w.finish())
}

pub fn decode(payload: &[u8]) -> Result<LinkTransition, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.class != ClassId::LinkTransition {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    if envelope.version != VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }

    let tag = r.u8()?;
    let trouble = r.u8()?;
    let first = r.u32()? as usize;
    let second = r.u32()? as usize;

    // Untracked instants are not invented on decode. `at_ns` is the field an operator reads.
    let state = match tag {
        STATE_DISCONNECTED => ConnectionState::Disconnected,
        STATE_CONNECTING => ConnectionState::Connecting,
        STATE_CONNECTED => ConnectionState::Connected { since: Moment::NotTracked },
        STATE_DEGRADED => ConnectionState::Degraded {
            since: Moment::NotTracked,
            reason: match trouble {
                TROUBLE_BACKLOGGED => DegradedReason::Backlogged { queued: first },
                TROUBLE_STALE => DegradedReason::Stale { last_message: Moment::NotTracked },
                TROUBLE_INCOMPLETE => {
                    DegradedReason::SubscriptionsIncomplete { attached: first, requested: second }
                }
                found => {
                    return Err(DecodeError::InvalidValue {
                        field: "trouble",
                        found: u64::from(found),
                    });
                }
            },
        },
        found => {
            return Err(DecodeError::InvalidValue { field: "state", found: u64::from(found) });
        }
    };

    let at_ns = r.i128()?;
    r.finish()?;
    Ok(LinkTransition { state, at_ns })
}

trait Pair {
    fn usize_pair(&mut self, a: usize, b: usize) -> &mut Self;
}

impl Pair for Writer {
    fn usize_pair(&mut self, a: usize, b: usize) -> &mut Self {
        self.u32(u32::try_from(a).unwrap_or(u32::MAX)).u32(u32::try_from(b).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
#[path = "../tests/unit/link/tests.rs"]
mod tests;
