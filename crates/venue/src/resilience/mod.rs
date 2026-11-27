// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What every venue adapter needs to survive an unreliable venue, written once: how long to wait
//! before trying again, when to stop trying, and how fast to ask.
//!
//! Each piece is a pure value. Time is passed in as nanoseconds and randomness is a trait the
//! caller supplies, so a session that uses them stays a function of its inputs and a replay of it
//! decides the same way.

pub mod backoff;
pub mod breaker;
pub mod budget;
pub mod gate;
pub mod jitter;

pub use backoff::{Backoff, BackoffError};
pub use breaker::{BreakerConfig, BreakerError, CircuitBreaker, CircuitState, Permit};
pub use budget::{BudgetError, Quota, RequestBudget};
pub use gate::{
    CallClass, CallGate, CallResult, GateConfig, GateError, Held, HttpReply, call_result_for_http,
    retry_after_from_header,
};
pub use jitter::{Fixed, Jitter, SplitMix64};
