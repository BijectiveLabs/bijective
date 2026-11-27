// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The simulated venue: a deterministic matching engine behind the venue execution contract.
//!
//! It matches against the recorded book under declared fill and latency models, keeps its own
//! orders through the platform's lifecycle, and reports what it holds as venue evidence, so a
//! research or paper run produces fills, positions and reconciliation through the same code as a
//! live run. It opens nothing and reads no clock. Design notes: `docs/concepts/simulated_venue.md`.

pub mod account;
pub mod book;
pub mod engine;
pub mod model;
pub mod venue;

pub use account::{Account, FeeRates, MONEY_SCALE};
pub use engine::{Engine, Restored};
pub use model::{FillModel, LatencyModel, MATCHING_VERSION, SimulationModel};
pub use venue::SimulatedVenue;

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
