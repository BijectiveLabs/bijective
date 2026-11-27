// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The health surface: readiness, reported accurately.
//! See `docs/concepts/observability.md`.

pub mod document;
pub mod surface;

pub use document::{Health, Readiness, ReadinessError};
pub use surface::{Surface, is_ready, read_at};

#[cfg(test)]
#[path = "../../tests/unit/readiness/tests.rs"]
mod tests;
