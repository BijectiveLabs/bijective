// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What became of a proposal, told back to the strategy that made it.
//!
//! The control engine writes one record for each control event about a proposal's order: refused
//! before admission, submitted, accepted, rejected, unknown, filled, cancel asked, canceled,
//! expired, reconciling. Each names the proposal by its position in the proposal stream, which is
//! the one identity the strategy engine already holds for what a strategy said. The control stream
//! stays the record. This stream is its projection for strategies, and every record names the
//! control position it mirrors so a mirror interrupted by a crash is completed on the next start.

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

pub const VERSION: u16 = 1;

/// The stream that mirrors control stream `control`.
#[must_use]
pub fn stream_for(control: &str) -> String {
    format!("{control}-feedback")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Refused before it was admitted: no order exists.
    Refused,
    /// Admitted and sent. The venue has not answered.
    Submitted,
    Accepted,
    Rejected,
    /// The answer could not settle whether the order reached the venue. Neither placed nor failed.
    Unknown,
    /// At the venue's scales, as it reported them.
    Filled {
        fill_id: String,
        quantity_scaled: i128,
        price_scaled: i128,
    },
    CancelRequested,
    Canceled,
    Expired,
    /// Evidence from the venue is being read to settle an unknown order.
    Reconciling,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    /// The position in the proposal stream of the proposal this is about.
    pub proposal_position: u64,
    /// Absent for a refusal: no order was made.
    pub client_id: Option<String>,
    pub outcome: Outcome,
    /// Why, for a refusal, a rejection or an unknown answer. Empty otherwise.
    pub reason: String,
    /// The control stream position this record mirrors.
    pub control_position: u64,
    /// When the control engine recorded it.
    pub at_ns: i128,
}

const REFUSED: u8 = 0;
const SUBMITTED: u8 = 1;
const ACCEPTED: u8 = 2;
const REJECTED: u8 = 3;
const UNKNOWN: u8 = 4;
const FILLED: u8 = 5;
const CANCEL_REQUESTED: u8 = 6;
const CANCELED: u8 = 7;
const EXPIRED: u8 = 8;
const RECONCILING: u8 = 9;

const ABSENT: u8 = 0;
const PRESENT: u8 = 1;

pub fn encode(feedback: &Feedback) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::OrderFeedback, VERSION);
    w.u64(feedback.proposal_position);
    match &feedback.client_id {
        Some(id) => {
            w.u8(PRESENT);
            w.str(id)?;
        }
        None => {
            w.u8(ABSENT);
        }
    }
    let code = match &feedback.outcome {
        Outcome::Refused => REFUSED,
        Outcome::Submitted => SUBMITTED,
        Outcome::Accepted => ACCEPTED,
        Outcome::Rejected => REJECTED,
        Outcome::Unknown => UNKNOWN,
        Outcome::Filled { .. } => FILLED,
        Outcome::CancelRequested => CANCEL_REQUESTED,
        Outcome::Canceled => CANCELED,
        Outcome::Expired => EXPIRED,
        Outcome::Reconciling => RECONCILING,
    };
    w.u8(code);
    if let Outcome::Filled { fill_id, quantity_scaled, price_scaled } = &feedback.outcome {
        w.str(fill_id)?;
        w.i128(*quantity_scaled).i128(*price_scaled);
    }
    w.str(&feedback.reason)?;
    w.u64(feedback.control_position).i128(feedback.at_ns);
    Ok(w.finish())
}

/// An unknown class, a newer version or an outcome this build does not know is refused.
pub fn decode(payload: &[u8]) -> Result<Feedback, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::OrderFeedback {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    let proposal_position = r.u64()?;
    let client_id = match r.u8()? {
        ABSENT => None,
        PRESENT => Some(r.str()?.to_string()),
        other => return Err(DecodeError::InvalidValue { field: "client_id", found: other.into() }),
    };
    let outcome = outcome(&mut r)?;
    let reason = r.str()?.to_string();
    let control_position = r.u64()?;
    let at_ns = r.i128()?;
    r.finish()?;
    Ok(Feedback { proposal_position, client_id, outcome, reason, control_position, at_ns })
}

fn outcome(r: &mut Reader<'_>) -> Result<Outcome, DecodeError> {
    Ok(match r.u8()? {
        REFUSED => Outcome::Refused,
        SUBMITTED => Outcome::Submitted,
        ACCEPTED => Outcome::Accepted,
        REJECTED => Outcome::Rejected,
        UNKNOWN => Outcome::Unknown,
        FILLED => Outcome::Filled {
            fill_id: r.str()?.to_string(),
            quantity_scaled: r.i128()?,
            price_scaled: r.i128()?,
        },
        CANCEL_REQUESTED => Outcome::CancelRequested,
        CANCELED => Outcome::Canceled,
        EXPIRED => Outcome::Expired,
        RECONCILING => Outcome::Reconciling,
        other => return Err(DecodeError::InvalidValue { field: "outcome", found: other.into() }),
    })
}

impl Outcome {
    /// The name a strategy reads.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Outcome::Refused => "refused",
            Outcome::Submitted => "submitted",
            Outcome::Accepted => "accepted",
            Outcome::Rejected => "rejected",
            Outcome::Unknown => "unknown",
            Outcome::Filled { .. } => "filled",
            Outcome::CancelRequested => "cancel_requested",
            Outcome::Canceled => "canceled",
            Outcome::Expired => "expired",
            Outcome::Reconciling => "reconciling",
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/signal/feedback_tests.rs"]
mod tests;
