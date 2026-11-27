// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reconciliation: what we hold against what the venue says, as of a stated position.
//!
//! No dependencies by design: a component that could reach a venue or a store could also write one.

pub mod classify;
pub mod record;
pub mod resolution;
pub mod signal;

pub use classify::{Divergence, DivergenceClass, MatchKind, ProfileDefect, Report, reconcile};
pub use record::{FallbackKey, Fidelity, PlatformPosition, VenuePosition};
pub use resolution::{Resolution, ResolutionError};
pub use signal::{OpenDivergence, Severity, open_signal};

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
