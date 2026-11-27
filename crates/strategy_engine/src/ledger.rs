// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The run ledger, made durable.
//! A run with no entry did not happen. Held in memory that rule would not survive a supervisor restart.

use std::collections::BTreeMap;

use crate::runs::ledger::Admitted;
use crate::runs::{RunLedgerEntry, ledger};
use bijective_event_store::digest::Digest;
use bijective_event_store::projection::Projection;
use bijective_event_store::store::{StoreError, StreamWriter};

/// The stream the supervisor records runs on.
pub const RUNS: &str = "strategy-runs";

#[derive(Debug)]
pub enum LedgerError {
    Encode(bijective_serialization::codec::writer::EncodeError),
    Store(StoreError),
    Decode(bijective_serialization::codec::DecodeError),
}

impl core::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LedgerError::Encode(e) => write!(f, "a run entry could not be encoded: {e:?}"),
            LedgerError::Store(e) => write!(f, "a run entry could not be recorded: {e:?}"),
            LedgerError::Decode(e) => write!(f, "a run entry could not be read: {e:?}"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Proof that a run is on the ledger. Only `record` makes one and `admit_result` asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    landed: u64,
}

impl Recorded {
    #[must_use]
    pub fn at(self) -> u64 {
        self.landed
    }
}

pub fn record(entry: &RunLedgerEntry, into: &mut StreamWriter) -> Result<Recorded, LedgerError> {
    let payload = ledger::encode(entry).map_err(LedgerError::Encode)?;
    let landed = into.append(&payload).map_err(LedgerError::Store)?;
    Ok(Recorded { landed })
}

/// Written after the admission because the fact exists only then. A field on the entry would be filled in later.
pub fn attribute(admitted: &Admitted, into: &mut StreamWriter) -> Result<u64, LedgerError> {
    let payload = ledger::encode_admitted(admitted).map_err(LedgerError::Encode)?;
    into.append(&payload).map_err(LedgerError::Store)
}

/// Two kinds, written at different moments: a run that admitted nothing has an entry and no admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerEvent {
    Ran(RunLedgerEntry),
    Admitted(Admitted),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    by_run: BTreeMap<String, Vec<RunLedgerEntry>>,
    admissions: BTreeMap<String, Vec<Admitted>>,
}

impl Ledger {
    /// A list, not a value: the same specification run twice is a replay, and research integrity compares the two.
    #[must_use]
    pub fn of(&self, specification: &Digest) -> &[RunLedgerEntry] {
        self.by_run.get(&specification.to_hex()).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn identities(&self) -> Vec<Digest> {
        self.by_run.values().filter_map(|e| e.first().map(|f| f.specification)).collect()
    }

    /// Looked up by strategy digest: a run's own identity cannot say which program it ran.
    #[must_use]
    pub fn of_strategy(&self, strategy: &Digest) -> Vec<(Digest, String)> {
        let mut found = Vec::new();
        for entries in self.by_run.values() {
            for entry in entries {
                if entry.strategy == Some(*strategy)
                    && !found.iter().any(|(run, _): &(Digest, String)| *run == entry.specification)
                {
                    found.push((entry.specification, entry.class.clone()));
                }
            }
        }
        found
    }

    /// Empty for no admissions or a run recorded before admissions existed: read it as nothing to measure.
    #[must_use]
    pub fn admissions_of(&self, specification: &Digest) -> &[Admitted] {
        self.admissions.get(&specification.to_hex()).map_or(&[], Vec::as_slice)
    }
}

impl Projection for Ledger {
    type Event = LedgerEvent;
    type Error = LedgerError;

    fn decode(payload: &[u8]) -> Result<LedgerEvent, LedgerError> {
        // The class decides: trying one decoder then the other could read a third class as whichever tolerates it.
        match bijective_serialization::codec::Reader::open(payload) {
            Ok((envelope, _))
                if envelope.class
                    == bijective_serialization::codec::ClassId::StrategyRunAdmitted =>
            {
                ledger::decode_admitted(payload)
                    .map(LedgerEvent::Admitted)
                    .map_err(LedgerError::Decode)
            }
            Ok(_) => ledger::decode(payload).map(LedgerEvent::Ran).map_err(LedgerError::Decode),
            Err(e) => Err(LedgerError::Decode(e)),
        }
    }

    fn apply(&mut self, event: &LedgerEvent) -> Result<(), LedgerError> {
        match event {
            LedgerEvent::Ran(entry) => {
                self.by_run.entry(entry.specification.to_hex()).or_default().push(entry.clone());
            }
            LedgerEvent::Admitted(admitted) => {
                self.admissions
                    .entry(admitted.specification.to_hex())
                    .or_default()
                    .push(admitted.clone());
            }
        }
        Ok(())
    }
}
