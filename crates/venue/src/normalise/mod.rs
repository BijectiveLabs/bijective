// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue decimal strings to platform scaled integers, without a float anywhere.

pub mod decimal;
pub mod outcome;

pub use decimal::{DecimalError, Scale, ScaleError, parse_scaled, render_scaled};
pub use outcome::{Attempted, Rejection, Unknown, VenueOutcome, terms_not_taken};

#[cfg(test)]
#[path = "../../tests/unit/normalise/tests.rs"]
mod tests;
