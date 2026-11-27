// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Drive one input sequence through the core, twice, and compare the bytes.
//! What it proves and does not prove: `docs/concepts/event_sourcing.md`.

use std::path::Path;
use std::sync::Arc;

use bijective_common::bus::Bus;
use bijective_common::clock::SequenceClock;
use bijective_event_store::store::StreamWriter;

/// One input: when it happens, on what topic, carrying what.
/// The time is the input's, not the host's. A wall-clock sequence would differ on every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub at_ns: i128,
    pub topic: String,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum DriveError {
    Clock(bijective_common::clock::ClockError),
    Bus(bijective_common::bus::BusError),
    Store(bijective_event_store::store::StoreError),
    Io(std::io::Error),
    /// A message the bus refused during a run. Continuing past one would compare a history that lost something.
    Refused {
        topic: String,
        sequence: u64,
    },
}

impl core::fmt::Display for DriveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Clock(e) => write!(f, "clock: {e}"),
            Self::Bus(e) => write!(f, "bus: {e}"),
            Self::Store(e) => write!(f, "store: {e:?}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Refused { topic, sequence } => {
                write!(f, "the bus refused {topic} position {sequence}")
            }
        }
    }
}

impl std::error::Error for DriveError {}

/// Frame one delivered message for the store.
/// The harness's own framing, not a registered event class. Fixed widths, big-endian, length-prefixed.
fn frame(topic: &str, sequence: u64, stamped_at_ns: i128, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(topic.len() + payload.len() + 32);
    out.extend_from_slice(&(topic.len() as u32).to_be_bytes());
    out.extend_from_slice(topic.as_bytes());
    out.extend_from_slice(&sequence.to_be_bytes());
    out.extend_from_slice(&stamped_at_ns.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Run a sequence through clock, bus and store, and return the bytes the store holds.
/// Topics are claimed and subscribed in first-mention order, so the run's shape follows the sequence.
pub fn drive(inputs: &[Input], root: &Path, stream: &str) -> Result<Vec<u8>, DriveError> {
    let started_at = inputs.first().map_or(0, |i| i.at_ns);
    let clock = Arc::new(SequenceClock::starting_at(started_at));
    let mut bus = Bus::new(clock.clone());
    let mut store = StreamWriter::open(root, stream).map_err(DriveError::Store)?;

    let mut writers = Vec::new();
    let mut readers = Vec::new();
    for input in inputs {
        if writers.iter().any(|(t, _): &(String, _)| t == &input.topic) {
            continue;
        }
        let writer = bus.claim(&input.topic).map_err(DriveError::Bus)?;
        let reader = bus.subscribe(&input.topic, inputs.len().max(1)).map_err(DriveError::Bus)?;
        writers.push((input.topic.clone(), writer));
        readers.push(reader);
    }

    for input in inputs {
        clock.advance_to(input.at_ns).map_err(DriveError::Clock)?;
        let writer = writers
            .iter()
            .find(|(t, _)| t == &input.topic)
            .map(|(_, w)| w)
            .expect("every topic was claimed above");

        let published = bus.publish(writer, &input.payload);
        if !published.reached_everyone() {
            return Err(DriveError::Refused {
                topic: published.topic,
                sequence: published.sequence,
            });
        }
    }

    // Subscription order, not map order, so the bytes do not depend on the build.
    for reader in &readers {
        for delivered in bus.drain(reader.id).map_err(DriveError::Bus)? {
            let framed = frame(
                &delivered.topic,
                delivered.sequence,
                delivered.stamped_at_ns,
                &delivered.payload,
            );
            store.append(&framed).map_err(DriveError::Store)?;
        }
    }

    read_stream_bytes(root, stream)
}

fn read_stream_bytes(root: &Path, stream: &str) -> Result<Vec<u8>, DriveError> {
    let dir = root.join(stream);
    let mut segments: Vec<_> = std::fs::read_dir(&dir)
        .map_err(DriveError::Io)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "seg"))
        .collect();
    segments.sort();

    let mut out = Vec::new();
    for segment in segments {
        out.extend_from_slice(&std::fs::read(segment).map_err(DriveError::Io)?);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
