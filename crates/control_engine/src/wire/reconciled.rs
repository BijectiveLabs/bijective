// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The layouts of `OrderReconciled` and `ReconciliationCompleted`: what venue evidence settled, and
//! the verdict one pass reached. Fills the evidence settled are written as `OrderFilled`.

use bijective_lifecycle::{OrderEvent, Verdict};
use bijective_serialization::codec::{DecodeError, Reader, Writer, writer::EncodeError};

use super::codes::read_flag;
use super::codes::{ABSENT, PRESENT};

const STARTED: u8 = 0;
const ACCEPTED: u8 = 1;
const REJECTED: u8 = 2;
const CANCELED: u8 = 3;
const EXPIRED: u8 = 4;

const READ: u8 = 0;
const NOT_PUBLISHED: u8 = 1;
const FAILED: u8 = 2;

const ALIGNED: u8 = 0;
const DIVERGED: u8 = 1;
const UNVERIFIED: u8 = 2;

/// How a pass's evidence read went. A failed read has said nothing about the venue and holds
/// resume. A venue that publishes no evidence is recorded and does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceRead {
    Read,
    NotPublished,
    Failed,
}

/// An order state change that venue evidence settled. The closed set reconciliation can apply
/// besides fills, so the stream cannot hold an evidence event the lifecycle never produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settlement {
    ReconciliationStarted,
    Accepted { venue_order_id: Option<String> },
    Rejected { reason: String },
    Canceled,
    Expired,
}

impl Settlement {
    /// `None` for a fill, written as its own class, and for events evidence never applies.
    #[must_use]
    pub fn of(event: &OrderEvent) -> Option<Self> {
        match event {
            OrderEvent::ReconciliationStarted => Some(Self::ReconciliationStarted),
            OrderEvent::Accepted { venue_order_id } => {
                Some(Self::Accepted { venue_order_id: venue_order_id.clone() })
            }
            OrderEvent::Rejected { reason } => Some(Self::Rejected { reason: reason.clone() }),
            OrderEvent::Canceled => Some(Self::Canceled),
            OrderEvent::Expired => Some(Self::Expired),
            _ => None,
        }
    }

    #[must_use]
    pub fn event(&self) -> OrderEvent {
        match self {
            Self::ReconciliationStarted => OrderEvent::ReconciliationStarted,
            Self::Accepted { venue_order_id } => {
                OrderEvent::Accepted { venue_order_id: venue_order_id.clone() }
            }
            Self::Rejected { reason } => OrderEvent::Rejected { reason: reason.clone() },
            Self::Canceled => OrderEvent::Canceled,
            Self::Expired => OrderEvent::Expired,
        }
    }
}

pub(crate) fn write_reconciled(
    w: &mut Writer,
    venue: &str,
    client_id: &str,
    settlement: &Settlement,
    at_ns: i128,
) -> Result<(), EncodeError> {
    write_settlement(w.str(venue)?.str(client_id)?, settlement)?;
    w.i128(at_ns);
    Ok(())
}

pub(crate) fn write_completed(
    w: &mut Writer,
    venue: &str,
    verdict: Verdict,
    findings: &[String],
    evidence: EvidenceRead,
    at_ns: i128,
) -> Result<(), EncodeError> {
    write_verdict(w.str(venue)?, verdict);
    write_findings(w, findings)?;
    w.i128(at_ns);
    w.u8(match evidence {
        EvidenceRead::Read => READ,
        EvidenceRead::NotPublished => NOT_PUBLISHED,
        EvidenceRead::Failed => FAILED,
    });
    Ok(())
}

fn write_settlement(w: &mut Writer, settlement: &Settlement) -> Result<(), EncodeError> {
    match settlement {
        Settlement::ReconciliationStarted => {
            w.u8(STARTED);
        }
        Settlement::Accepted { venue_order_id } => {
            w.u8(ACCEPTED);
            match venue_order_id {
                Some(id) => {
                    w.u8(PRESENT).str(id)?;
                }
                None => {
                    w.u8(ABSENT);
                }
            }
        }
        Settlement::Rejected { reason } => {
            w.u8(REJECTED).str(reason)?;
        }
        Settlement::Canceled => {
            w.u8(CANCELED);
        }
        Settlement::Expired => {
            w.u8(EXPIRED);
        }
    }
    Ok(())
}

pub(crate) fn read_settlement(r: &mut Reader<'_>) -> Result<Settlement, DecodeError> {
    Ok(match r.u8()? {
        STARTED => Settlement::ReconciliationStarted,
        ACCEPTED => {
            let venue_order_id =
                if read_flag(r, "venue_order_id")? { Some(r.str()?.to_string()) } else { None };
            Settlement::Accepted { venue_order_id }
        }
        REJECTED => Settlement::Rejected { reason: r.str()?.to_string() },
        CANCELED => Settlement::Canceled,
        EXPIRED => Settlement::Expired,
        found => {
            return Err(DecodeError::InvalidValue { field: "settlement", found: u64::from(found) });
        }
    })
}

fn write_verdict(w: &mut Writer, verdict: Verdict) {
    w.u8(match verdict {
        Verdict::Aligned => ALIGNED,
        Verdict::Diverged => DIVERGED,
        Verdict::Unverified => UNVERIFIED,
    });
}

pub(crate) fn read_verdict(r: &mut Reader<'_>) -> Result<Verdict, DecodeError> {
    match r.u8()? {
        ALIGNED => Ok(Verdict::Aligned),
        DIVERGED => Ok(Verdict::Diverged),
        UNVERIFIED => Ok(Verdict::Unverified),
        found => Err(DecodeError::InvalidValue { field: "verdict", found: u64::from(found) }),
    }
}

fn write_findings(w: &mut Writer, findings: &[String]) -> Result<(), EncodeError> {
    w.u32(u32::try_from(findings.len()).unwrap_or(u32::MAX));
    for finding in findings.iter().take(u32::MAX as usize) {
        w.str(finding)?;
    }
    Ok(())
}

/// Appended after the time. A record written before it names none and reads as `Read`, since
/// only a completed pass was recorded then.
pub(crate) fn read_evidence(r: &mut Reader<'_>) -> Result<EvidenceRead, DecodeError> {
    if !r.more() {
        return Ok(EvidenceRead::Read);
    }
    match r.u8()? {
        READ => Ok(EvidenceRead::Read),
        NOT_PUBLISHED => Ok(EvidenceRead::NotPublished),
        FAILED => Ok(EvidenceRead::Failed),
        found => Err(DecodeError::InvalidValue { field: "evidence", found: u64::from(found) }),
    }
}

pub(crate) fn read_findings(r: &mut Reader<'_>) -> Result<Vec<String>, DecodeError> {
    let count = r.u32()?;
    (0..count).map(|_| r.str().map(str::to_string)).collect()
}
