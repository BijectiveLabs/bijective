// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Bounded randomness, supplied by the caller.
//!
//! Retries that all wait the same time arrive together, so a venue that recovers is knocked over
//! again by every client at once. Spreading the wait needs randomness, and randomness read from the
//! host would make a session's deadlines unrepeatable. A source is therefore a value the caller
//! owns: seeded in production, fixed in a test.

/// A source of bounded random numbers.
pub trait Jitter: Send + std::fmt::Debug {
    /// A value in `0..bound`. A bound of zero is treated as one, so the answer is always zero.
    fn below(&mut self, bound: u64) -> u64;
}

/// SplitMix64: a small generator whose whole state is its seed, so a seed fixes the sequence.
/// Not for secrets. It spreads retries. It protects nothing.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Seeded from the clock a program was given and its process number, so two processes that
    /// start in the same nanosecond still retry apart. The only impure line is the process number,
    /// read once at the boundary. A session built from a fixed clock in one process is repeatable.
    #[must_use]
    pub fn for_this_process(now_ns: i128) -> Self {
        let clock_bits = u64::try_from(now_ns & i128::from(u64::MAX)).unwrap_or(0);
        Self::seeded(clock_bits ^ (u64::from(std::process::id()) << 32))
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Jitter for SplitMix64 {
    /// Draws are rejected above the largest multiple of `bound`, so no value is favoured.
    fn below(&mut self, bound: u64) -> u64 {
        let bound = bound.max(1);
        let limit = u64::MAX - (u64::MAX % bound);
        loop {
            let draw = self.next();
            if draw < limit {
                return draw % bound;
            }
        }
    }
}

/// Always the same answer, clamped into the bound. For tests that pin an edge of a wait, and for
/// a run that must not spread its retries.
#[derive(Debug, Clone, Copy)]
pub struct Fixed(pub u64);

impl Jitter for Fixed {
    fn below(&mut self, bound: u64) -> u64 {
        self.0.min(bound.saturating_sub(1))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resilience/jitter_tests.rs"]
mod tests;
