// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Where a pre-registration lives, and how one is read back.
//! Research integrity: the registration registry. Append-only and hash-linked, so nothing is deleted.

use std::path::Path;

use bijective_event_store::digest::Digest;
use bijective_event_store::store::{StoreError, StreamWriter, read_range};
use bijective_serialization::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};

use crate::evidence::preregistration::{DecisionRule, Direction, Horizon, PreRegistration};
use crate::evidence::walk_forward::{Fold, Positions, WalkForward};

/// Layout version. V2 appends the horizon and v3 the folds. Older records read
/// as `NotDeclared` and no folds.
pub const VERSION: u16 = 3;

const HORIZON_AFTER: u8 = 0;
const HORIZON_NOT_HELD: u8 = 1;

pub const REGISTRATIONS: &str = "registrations";

const DIRECTION_AT_LEAST: u8 = 0;
const DIRECTION_AT_MOST: u8 = 1;

#[derive(Debug)]
pub enum RegistryError {
    Encode(EncodeError),
    Decode(DecodeError),
    Store(StoreError),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Encode(e) => write!(f, "encode: {e:?}"),
            RegistryError::Decode(e) => write!(f, "decode: {e}"),
            RegistryError::Store(e) => write!(f, "store: {e:?}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<StoreError> for RegistryError {
    fn from(e: StoreError) -> Self {
        RegistryError::Store(e)
    }
}

/// The verdict is not stored. It is computed from this and what a run measured.
pub fn encode(registration: &PreRegistration, at_ns: i128) -> Result<Vec<u8>, RegistryError> {
    let mut w = Writer::new(ClassId::PreRegistrationDeclared, VERSION);
    w.bytes(&registration.run().0).map_err(RegistryError::Encode)?;
    w.str(registration.hypothesis()).map_err(RegistryError::Encode)?;
    w.str(&registration.rule().measure).map_err(RegistryError::Encode)?;
    w.u8(match registration.rule().direction {
        Direction::AtLeast => DIRECTION_AT_LEAST,
        Direction::AtMost => DIRECTION_AT_MOST,
    });
    w.i128(registration.rule().threshold);
    w.i128(at_ns);
    match registration.horizon() {
        Horizon::After { positions } => {
            w.u8(HORIZON_AFTER).u64(positions);
        }
        // A claim that holds nothing writes a zero it never reads back, so the layout has one shape.
        Horizon::NotHeld | Horizon::NotDeclared => {
            w.u8(HORIZON_NOT_HELD).u64(0);
        }
    }
    // A zero count reads back as none declared, not as a structure over nothing.
    let folds = registration.held_out().map_or(&[][..], WalkForward::folds);
    w.u32(u32::try_from(folds.len()).unwrap_or(u32::MAX));
    for fold in folds {
        w.u64(fold.learn.from).u64(fold.learn.until).u64(fold.test.from).u64(fold.test.until);
    }
    Ok(w.finish())
}

/// One registration as written. The instant is for operators. The identity holds the before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    pub run: Digest,
    pub hypothesis: String,
    pub rule: DecisionRule,
    pub at_ns: i128,
    pub horizon: Horizon,
    /// The folds the claim's result is judged on. Empty is *none declared*.
    pub folds: Vec<Fold>,
}

pub fn decode(payload: &[u8]) -> Result<Declared, DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    // Refused before any field is read so the error names the real fault.
    if envelope.class != ClassId::PreRegistrationDeclared {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }

    let run = r.bytes()?;
    let run: [u8; 32] = run
        .try_into()
        .map_err(|_| DecodeError::InvalidValue { field: "registration.run", found: 0 })?;
    let hypothesis = r.str()?.to_string();
    let measure = r.str()?.to_string();
    let direction = match r.u8()? {
        DIRECTION_AT_LEAST => Direction::AtLeast,
        DIRECTION_AT_MOST => Direction::AtMost,
        other => {
            return Err(DecodeError::InvalidValue {
                field: "registration.direction",
                found: u64::from(other),
            });
        }
    };
    let threshold = r.i128()?;
    let at_ns = r.i128()?;
    // Appended at v2. A missing horizon is `NotDeclared`, not a claim that holds nothing.
    let horizon = if r.more() {
        match r.u8()? {
            HORIZON_AFTER => Horizon::After { positions: r.u64()? },
            HORIZON_NOT_HELD => {
                let _ = r.u64()?;
                Horizon::NotHeld
            }
            other => {
                return Err(DecodeError::InvalidValue {
                    field: "registration.horizon",
                    found: u64::from(other),
                });
            }
        }
    } else {
        Horizon::NotDeclared
    };

    // Appended at v3. Absent means no structure declared.
    let mut folds = Vec::new();
    if r.more() {
        let count = r.u32()?;
        for _ in 0..count {
            folds.push(Fold {
                learn: Positions::new(r.u64()?, r.u64()?),
                test: Positions::new(r.u64()?, r.u64()?),
            });
        }
    }
    r.finish()?;

    Ok(Declared {
        run: Digest(run),
        hypothesis,
        rule: DecisionRule { measure, direction, threshold },
        at_ns,
        horizon,
        folds,
    })
}

/// Declare a claim before its run. The append-only hash link holds the before.
pub fn declare(
    registration: &PreRegistration,
    at_ns: i128,
    into: &mut StreamWriter,
) -> Result<u64, RegistryError> {
    let payload = encode(registration, at_ns)?;
    into.append(&payload).map_err(RegistryError::Store)
}

impl From<&Declared> for PreRegistration {
    /// Only reachable through a `Declared`, so it cannot mint a claim for an unregistered run.
    fn from(declared: &Declared) -> Self {
        // Not re-checked: the window is not in the record, and rebuilding one would invent a bound.
        let held_out =
            (!declared.folds.is_empty()).then(|| WalkForward::as_read(declared.folds.clone()));

        PreRegistration::as_read(
            declared.run,
            declared.hypothesis.clone(),
            declared.rule.clone(),
            declared.horizon,
            held_out,
        )
    }
}

/// The registration standing for `run`. Latest wins and earlier ones stay in the stream. A claim
/// never carries across specifications.
pub fn standing(root: &Path, stream: &str, run: Digest) -> Result<Option<Declared>, RegistryError> {
    let entries = read_range(root, stream, 0, u64::MAX)?;

    let mut found = None;
    for entry in entries {
        // Refused, never skipped: an unreadable entry may be the one being asked for.
        let declared = decode(&entry.payload).map_err(RegistryError::Decode)?;
        if declared.run == run {
            found = Some(declared);
        }
    }

    Ok(found)
}
