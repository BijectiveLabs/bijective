// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The deployment kit: one manifest in, units and an artefact check out.
//!
//! See `docs/concepts/deployment.md`.

pub mod artefact;
pub mod compose;
pub mod manifest;
pub mod notices;
pub mod sequencer;
pub mod units;

pub use artefact::{ArtefactError, Checksums, verify_artefact};
pub use compose::compose;
pub use manifest::{Deployment, ManifestError, Program, ProgramClass};
pub use notices::{
    Attested, NoticesError, collect_texts, distinct_licences, parse_attestation, render,
};
pub use sequencer::{Node, NodeState, ProgramState, assess_node};
pub use units::{Generated, generate};

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

pub mod host;
