// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Durable limit state: declarations, consumption and releases as events, budget as a replay.

pub mod state;
pub mod wire;

pub use state::{Budget, Holding, LimitState, Limits, Recovery, StateError};
pub use wire::{LimitEvent, ReleaseReason, VERSION, decode, encode};

#[cfg(test)]
#[path = "../../tests/unit/limit_state/tests.rs"]
mod tests;
