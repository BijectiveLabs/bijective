// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Event store - append-only segments, hash-chained, sealed.
//! Design and rationale: `docs/concepts/event_sourcing.md`.

pub mod cursor;
pub mod digest;
pub mod format;
pub mod retention;
pub mod store;

#[cfg(test)]
#[path = "../tests/unit/cursor_tests.rs"]
mod cursor_tests;
#[cfg(test)]
#[path = "../tests/unit/digest_tests.rs"]
mod digest_tests;
#[cfg(test)]
#[path = "../tests/unit/format_tests.rs"]
mod format_tests;
#[cfg(test)]
#[path = "../tests/unit/store_tests.rs"]
mod store_tests;

pub mod projection;
