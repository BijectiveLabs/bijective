// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What happened to a run, recorded before its output is usable.
//! A run with no entry did not happen, and its output is refused.

use bijective_event_store::digest::Digest;
use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Completed {
        status: i32,
    },
    LimitExceeded,
    Timeout,
    /// It never started, because the host could not bound it.
    Refused,
}

impl ExitReason {
    /// A cut-short run emitted whatever it had reached. A partial pass read as a result is how an underpowered verdict becomes a promotion.
    #[must_use]
    pub fn output_is_usable(self) -> bool {
        matches!(self, ExitReason::Completed { status: 0 })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLedgerEntry {
    pub specification: Digest,
    pub class: String,
    pub started_ns: i128,
    pub ended_ns: i128,
    pub exit: ExitReason,
    /// The program an operator named. A digest cannot be turned back into a name. `None` predates version 3.
    pub program: Option<String>,
    /// Which version of the program ran. Not the identity: a stage sequence must not carry a clearance across two digests under one name.
    /// `None` predates version 4.
    pub content: Option<Digest>,
    /// The strategy the run was of. `None` predates the field, and a stage cannot be cleared on one.
    pub strategy: Option<Digest>,
    /// A digest over what the run emitted, so a replay is compared without keeping both outputs.
    pub emitted: Digest,
}

/// Newest layout this build reads. Appended: 2 strategy, 3 program, 4 content.
pub const VERSION: u16 = 4;

const COMPLETED: u8 = 0;
const LIMIT_EXCEEDED: u8 = 1;
const TIMEOUT: u8 = 2;
const REFUSED: u8 = 3;

pub fn encode(entry: &RunLedgerEntry) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::StrategyRunRecorded, VERSION);
    w.bytes(&entry.specification.0)?;
    w.str(&entry.class)?;
    w.i128(entry.started_ns);
    w.i128(entry.ended_ns);
    match entry.exit {
        ExitReason::Completed { status } => {
            w.u8(COMPLETED).i64(i64::from(status));
        }
        // The status is written for every reason so the layout has one shape. Reasons without one write a zero never read back.
        ExitReason::LimitExceeded => {
            w.u8(LIMIT_EXCEEDED).i64(0);
        }
        ExitReason::Timeout => {
            w.u8(TIMEOUT).i64(0);
        }
        ExitReason::Refused => {
            w.u8(REFUSED).i64(0);
        }
    }
    w.bytes(&entry.emitted.0)?;
    match entry.strategy {
        Some(strategy) => {
            w.u8(1);
            w.bytes(&strategy.0)?;
        }
        None => {
            w.u8(0);
        }
    }
    match &entry.program {
        Some(program) => {
            w.u8(1);
            w.str(program)?;
        }
        None => {
            w.u8(0);
        }
    }
    match entry.content {
        Some(content) => {
            w.u8(1);
            w.bytes(&content.0)?;
        }
        None => {
            w.u8(0);
        }
    }
    Ok(w.finish())
}

/// Decode an entry. An unknown class or a newer version is refused, never skipped.
pub fn decode(payload: &[u8]) -> Result<RunLedgerEntry, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::StrategyRunRecorded {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let specification = digest_from(r.bytes()?, "specification")?;
    let class = r.str()?.to_string();
    let started_ns = r.i128()?;
    let ended_ns = r.i128()?;
    let reason = r.u8()?;
    let status = r.i64()?;
    let exit = match reason {
        COMPLETED => ExitReason::Completed {
            status: i32::try_from(status).map_err(|_| DecodeError::InvalidValue {
                field: "status",
                found: status.unsigned_abs(),
            })?,
        },
        LIMIT_EXCEEDED => ExitReason::LimitExceeded,
        TIMEOUT => ExitReason::Timeout,
        REFUSED => ExitReason::Refused,
        other => {
            return Err(DecodeError::InvalidValue { field: "exit", found: u64::from(other) });
        }
    };
    let emitted = digest_from(r.bytes()?, "emitted")?;
    let strategy =
        if r.more() && r.u8()? == 1 { Some(digest_from(r.bytes()?, "strategy")?) } else { None };
    // Appended at version 3: absent means written before the ledger could say, not a nameless strategy.
    let program = if r.more() && r.u8()? == 1 { Some(r.str()?.to_string()) } else { None };
    // Appended at version 4: absent means the bytes that ran were not recorded, not a program with no content.
    let content =
        if r.more() && r.u8()? == 1 { Some(digest_from(r.bytes()?, "content")?) } else { None };

    Ok(RunLedgerEntry {
        specification,
        class,
        started_ns,
        ended_ns,
        exit,
        emitted,
        strategy,
        program,
        content,
    })
}

fn digest_from(raw: &[u8], field: &'static str) -> Result<Digest, DecodeError> {
    let mut bytes = [0u8; 32];
    if raw.len() != bytes.len() {
        return Err(DecodeError::InvalidValue { field, found: raw.len() as u64 });
    }
    bytes.copy_from_slice(raw);
    Ok(Digest(bytes))
}

/// Where a run's proposals landed, and what market the strategy had been sent when they did.
/// The link a measurement rests on: an operator's own timestamp is not a fact the platform can measure against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    pub specification: Digest,
    /// The first and last proposal-stream positions this admission wrote, inclusive.
    pub from: u64,
    pub through: u64,
    /// Carried beside the range: a reader can tell a contiguous block from one another writer interleaved with.
    pub count: u64,
    /// A paper session writes elsewhere (`Stage::proposals`), so assuming one stream would measure the wrong proposals.
    pub proposals: String,
    /// The market stream, carried not assumed: a position is a number on one particular stream.
    pub market: String,
    pub market_through: u64,
}

/// Layout version for an admission. Version 2 appended the proposal stream.
pub const ADMITTED_VERSION: u16 = 2;

pub fn encode_admitted(admitted: &Admitted) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::StrategyRunAdmitted, ADMITTED_VERSION);
    w.bytes(&admitted.specification.0)?;
    w.u64(admitted.from);
    w.u64(admitted.through);
    w.u64(admitted.count);
    w.str(&admitted.market)?;
    w.u64(admitted.market_through);
    w.str(&admitted.proposals)?;
    Ok(w.finish())
}

pub fn decode_admitted(payload: &[u8]) -> Result<Admitted, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > ADMITTED_VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: ADMITTED_VERSION,
        });
    }
    if envelope.class != ClassId::StrategyRunAdmitted {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    Ok(Admitted {
        specification: digest_from(r.bytes()?, "specification")?,
        from: r.u64()?,
        through: r.u64()?,
        count: r.u64()?,
        market: r.str()?.to_string(),
        market_through: r.u64()?,
        // Appended at version 2. A record without it named the one stream that existed then.
        proposals: if r.more() { r.str()?.to_string() } else { "proposals".to_string() },
    })
}

impl Admitted {
    /// A range another writer interleaved with would attribute another run's proposals to this run.
    #[must_use]
    pub fn is_contiguous(&self) -> bool {
        self.through.saturating_sub(self.from) + 1 == self.count
    }
}
