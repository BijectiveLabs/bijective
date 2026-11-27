// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The Data Engine logic, separated from the process that runs it.
//! It does not decode a venue. The market layout lives in `crates/serialization`.

pub use bijective_serialization::signal::market::{BookUpdate, VERSION, decode, encode};

pub mod declared;
pub mod import;
pub mod journal;
pub mod link;
pub mod recorder;
pub mod retention;

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
