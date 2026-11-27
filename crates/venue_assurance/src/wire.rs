// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a check concluded, recorded so a later decision can rest on it (venue assurance).

use std::path::Path;

use bijective_event_store::digest::{Digest, sha256};
use bijective_event_store::store::{StoreError, StreamWriter, read_range};
use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

use crate::{AssuranceReport, NotObservedReason, Verification};

/// Layout version written and the newest read. Version 2 appends the evidence digest.
pub const VERSION: u16 = 2;

/// The stream a venue's checks are recorded on.
pub const ASSURANCE: &str = "venue-assurance";

const CONFIRMED: u8 = 0;
const DIVERGED: u8 = 1;
const NOT_OBSERVED: u8 = 2;

const UNREACHABLE: u8 = 0;
const METHOD_GONE: u8 = 1;

/// What a check concluded about one venue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Concluded {
    Confirmed,
    Diverged {
        findings: u32,
        /// Whether any finding could make the platform act on a guarantee the venue never made.
        capital_relevant: bool,
    },
    NotObserved {
        reason: NotObservedReason,
    },
}

/// One check, as the platform records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub venue: String,
    pub concluded: Concluded,
    /// When the venue was checked. Never when this was written.
    pub at_ns: i128,
    /// Digest of the venue answer the conclusion was drawn from. `None` is a hand-written or pre-runner record. A fee may not rest on one.
    pub evidence: Option<Digest>,
}

impl Outcome {
    #[must_use]
    pub fn of(report: &AssuranceReport) -> Self {
        let concluded = match &report.verification {
            Verification::Confirmed { .. } => Concluded::Confirmed,
            Verification::Diverged { findings, .. } => Concluded::Diverged {
                findings: findings.len() as u32,
                capital_relevant: findings.iter().any(super::Divergence::is_capital_relevant),
            },
            Verification::NotObserved { reason, .. } => Concluded::NotObserved { reason: *reason },
        };
        Self {
            venue: report.venue.clone(),
            concluded,
            at_ns: report.verification.at_ns(),
            evidence: None,
        }
    }

    /// The same conclusion, tied to the venue's answer it was drawn from.
    #[must_use]
    pub fn drawn_from(mut self, answer: &[u8]) -> Self {
        self.evidence = Some(sha256(answer));
        self
    }

    /// Only a confirmation: no answer is not a confirmation and a divergence is not a pass.
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        matches!(self.concluded, Concluded::Confirmed)
    }
}

#[derive(Debug)]
pub enum AssuranceRecordError {
    Encode(EncodeError),
    Decode(DecodeError),
    Store(StoreError),
}

impl core::fmt::Display for AssuranceRecordError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AssuranceRecordError::Encode(e) => write!(f, "the outcome could not be encoded: {e:?}"),
            AssuranceRecordError::Decode(e) => write!(f, "the outcome could not be read: {e:?}"),
            AssuranceRecordError::Store(e) => write!(f, "store: {e:?}"),
        }
    }
}

impl std::error::Error for AssuranceRecordError {}

pub fn encode(outcome: &Outcome) -> Result<Vec<u8>, AssuranceRecordError> {
    let mut w = Writer::new(ClassId::VenueAssuranceRecorded, VERSION);
    w.str(&outcome.venue).map_err(AssuranceRecordError::Encode)?;
    match &outcome.concluded {
        Concluded::Confirmed => {
            w.u8(CONFIRMED).u32(0).u8(0).u8(0);
        }
        Concluded::Diverged { findings, capital_relevant } => {
            w.u8(DIVERGED).u32(*findings).u8(u8::from(*capital_relevant)).u8(0);
        }
        Concluded::NotObserved { reason } => {
            w.u8(NOT_OBSERVED).u32(0).u8(0).u8(match reason {
                NotObservedReason::VenueUnreachable => UNREACHABLE,
                NotObservedReason::RetrievalMethodGone => METHOD_GONE,
            });
        }
    }
    w.i128(outcome.at_ns);
    match outcome.evidence {
        Some(digest) => {
            w.u8(1);
            w.bytes(&digest.0).map_err(AssuranceRecordError::Encode)?;
        }
        None => {
            w.u8(0);
        }
    }
    Ok(w.finish())
}

pub fn decode(payload: &[u8]) -> Result<Outcome, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::VenueAssuranceRecorded {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let venue = r.str()?.to_string();
    let kind = r.u8()?;
    let findings = r.u32()?;
    let capital_relevant = r.u8()?;
    let reason = r.u8()?;
    let concluded = match kind {
        CONFIRMED => Concluded::Confirmed,
        DIVERGED => Concluded::Diverged { findings, capital_relevant: capital_relevant != 0 },
        NOT_OBSERVED => Concluded::NotObserved {
            reason: match reason {
                UNREACHABLE => NotObservedReason::VenueUnreachable,
                METHOD_GONE => NotObservedReason::RetrievalMethodGone,
                other => {
                    return Err(DecodeError::InvalidValue {
                        field: "assurance.reason",
                        found: u64::from(other),
                    });
                }
            },
        },
        other => {
            return Err(DecodeError::InvalidValue {
                field: "assurance.concluded",
                found: u64::from(other),
            });
        }
    };

    let at_ns = r.i128()?;
    // Appended at version 2. Older records carry no evidence.
    let evidence = if r.more() && r.u8()? == 1 {
        let raw = r.bytes()?;
        let mut bytes = [0u8; 32];
        if raw.len() != bytes.len() {
            return Err(DecodeError::InvalidValue {
                field: "assurance.evidence",
                found: raw.len() as u64,
            });
        }
        bytes.copy_from_slice(raw);
        Some(Digest(bytes))
    } else {
        None
    };

    Ok(Outcome { venue, concluded, at_ns, evidence })
}

/// Record what a check concluded.
pub fn record(outcome: &Outcome, into: &mut StreamWriter) -> Result<u64, AssuranceRecordError> {
    let payload = encode(outcome)?;
    into.append(&payload).map_err(AssuranceRecordError::Store)
}

/// The latest check for a venue, or none. Earlier outcomes stay so a lapsed confirmation stays visible.
pub fn standing(root: &Path, venue: &str) -> Result<Option<Outcome>, AssuranceRecordError> {
    let entries = read_range(root, ASSURANCE, 0, u64::MAX).map_err(AssuranceRecordError::Store)?;

    let mut found = None;
    for entry in entries {
        let outcome = decode(&entry.payload).map_err(AssuranceRecordError::Decode)?;
        if outcome.venue == venue {
            found = Some(outcome);
        }
    }
    Ok(found)
}

/// Why a fee schedule could not be declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationRefused {
    /// Nothing has checked this venue, as opposed to a check that disagreed.
    NeverChecked { venue: String },
    /// The check that stands is not a confirmation.
    NotConfirmed { venue: String, concluded: Concluded, at_ns: i128 },
    /// The standing check has no digest of a venue answer. Closes the hand-written path.
    NotDrawnFromEvidence { venue: String, at_ns: i128 },
}

impl core::fmt::Display for DeclarationRefused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DeclarationRefused::NeverChecked { venue } => {
                write!(f, "nothing has checked {venue}, so its fees rest on nothing")
            }
            DeclarationRefused::NotConfirmed { venue, concluded, at_ns } => write!(
                f,
                "the check standing for {venue} at {at_ns} is {concluded:?}, and a fee may not rest on it"
            ),
            DeclarationRefused::NotDrawnFromEvidence { venue, at_ns } => write!(
                f,
                "the check standing for {venue} at {at_ns} was drawn from nothing the platform kept"
            ),
        }
    }
}

impl std::error::Error for DeclarationRefused {}

#[derive(Debug)]
pub enum DeclareError {
    Refused(DeclarationRefused),
    Assurance(AssuranceRecordError),
    Fees(bijective_serialization::signal::fees::FeesError),
}

impl core::fmt::Display for DeclareError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DeclareError::Refused(r) => write!(f, "{r}"),
            DeclareError::Assurance(e) => write!(f, "{e}"),
            DeclareError::Fees(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DeclareError {}

/// Declare what a venue charges, only if a check confirms that venue.
/// Opens the stream itself, only where it writes: a lock held through a refusal stays held on exit.
pub fn declare_fees(
    root: &Path,
    fees: &bijective_serialization::signal::fees::Fees,
) -> Result<u64, DeclareError> {
    match standing(root, &fees.venue).map_err(DeclareError::Assurance)? {
        None => Err(DeclareError::Refused(DeclarationRefused::NeverChecked {
            venue: fees.venue.clone(),
        })),
        Some(outcome) if !outcome.is_confirmed() => {
            Err(DeclareError::Refused(DeclarationRefused::NotConfirmed {
                venue: fees.venue.clone(),
                concluded: outcome.concluded,
                at_ns: outcome.at_ns,
            }))
        }
        Some(outcome) if outcome.evidence.is_none() => {
            Err(DeclareError::Refused(DeclarationRefused::NotDrawnFromEvidence {
                venue: fees.venue.clone(),
                at_ns: outcome.at_ns,
            }))
        }
        Some(_) => {
            let payload =
                bijective_serialization::signal::fees::encode(fees).map_err(DeclareError::Fees)?;
            let mut into = StreamWriter::open(root, bijective_serialization::signal::fees::FEES)
                .map_err(|e| DeclareError::Assurance(AssuranceRecordError::Store(e)))?;
            into.append(&payload)
                .map_err(|e| DeclareError::Assurance(AssuranceRecordError::Store(e)))
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/wire_tests.rs"]
mod wire_tests;
