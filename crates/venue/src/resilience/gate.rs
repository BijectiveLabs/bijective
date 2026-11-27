// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The limits a venue client applies to its own calls, in one place so both adapters apply the same.
//!
//! A submit and a read each have a circuit breaker, so a failing order path does not stop reads and
//! a failing read path does not stop orders. Reads also draw on a request quota. A cancel is never
//! held: an exit must always be attempted. Submits are not charged to the read quota because the
//! control engine already bounds them by the venue's order rate.

use super::breaker::{BreakerConfig, BreakerError, CircuitBreaker, Permit};
use super::budget::{BudgetError, Quota, RequestBudget};

/// What a call is for, which decides which limits apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallClass {
    Submit,
    Cancel,
    Read,
}

/// Why a call was held back before it was made. Nothing reached the venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The breaker for this path is open. Ask again at `retry_at_ns`.
    CircuitOpen { retry_at_ns: i128 },
    /// The read quota is spent, or the venue asked for patience. Ask again in `wait_ns`.
    OverBudget { wait_ns: i128 },
}

/// The longest a venue's own `Retry-After` is believed. A broken or hostile header must not hold
/// reads for days. A venue that means longer is asked again after this.
const LONGEST_RETRY_AFTER_NS: i128 = 3_600_000_000_000;

/// An HTTP answer reduced to what the gate and the outcome readers need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    /// Zero when no answer came: the request may or may not have arrived.
    pub status: u16,
    pub body: String,
    pub retry_after_ns: Option<i128>,
}

/// A `Retry-After` given in whole seconds. The date form, zero, negative and malformed values say
/// nothing the gate can use. Read on the venue's behalf, so it is capped.
#[must_use]
pub fn retry_after_from_header(value: &str) -> Option<i128> {
    let digits = value.trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds = digits.parse::<u64>().ok().map_or(i128::MAX, i128::from);
    let wait_ns = seconds.saturating_mul(1_000_000_000).min(LONGEST_RETRY_AFTER_NS);
    (wait_ns > 0).then_some(wait_ns)
}

/// What an HTTP status says about the venue's health. A refusal of the request itself is an
/// answer, so the venue is healthy. No answer or a server error is not. 429 asks for patience.
#[must_use]
pub fn call_result_for_http(status: u16, retry_after_ns: Option<i128>) -> CallResult {
    match status {
        0 | 500..=599 => CallResult::Unhealthy,
        429 => CallResult::Throttled { retry_after_ns },
        _ => CallResult::Healthy,
    }
}

/// What the venue's answer says about its health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallResult {
    /// The venue answered, whether it accepted the request or refused it.
    Healthy,
    /// A transport failure, a timeout or a server error: the venue is not working.
    Unhealthy,
    /// The venue is alive and asked for fewer requests, and may have said for how long.
    Throttled { retry_after_ns: Option<i128> },
}

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CircuitOpen { retry_at_ns } => {
                write!(f, "the circuit is open until {retry_at_ns} ns on the platform clock")
            }
            Self::OverBudget { wait_ns } => write!(f, "over the request quota; wait {wait_ns} ns"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateConfig {
    /// Applied to the submit path and to the read path, each with its own count.
    pub breaker: BreakerConfig,
    pub reads: Quota,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    Breaker(BreakerError),
    Budget(BudgetError),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Breaker(e) => write!(f, "circuit breaker: {e}"),
            Self::Budget(e) => write!(f, "read quota: {e}"),
        }
    }
}

impl std::error::Error for GateError {}

#[derive(Debug, Clone)]
pub struct CallGate {
    submit: CircuitBreaker,
    read: CircuitBreaker,
    reads: RequestBudget,
}

impl CallGate {
    pub fn new(config: GateConfig) -> Result<Self, GateError> {
        Ok(Self {
            submit: CircuitBreaker::new(config.breaker).map_err(GateError::Breaker)?,
            read: CircuitBreaker::new(config.breaker).map_err(GateError::Breaker)?,
            reads: RequestBudget::new(config.reads).map_err(GateError::Budget)?,
        })
    }

    /// Asks to make a call. An `Ok` for a submit or a read must be followed by `report`.
    pub fn admit(&mut self, class: CallClass, now_ns: i128) -> Result<(), Held> {
        match class {
            CallClass::Cancel => Ok(()),
            CallClass::Submit => permit(&mut self.submit, now_ns),
            CallClass::Read => {
                // The quota first: a refusal by the breaker after it would waste a token, while a
                // quota refusal after the breaker would strand a probe.
                self.reads.acquire(now_ns).map_err(|wait_ns| Held::OverBudget { wait_ns })?;
                permit(&mut self.read, now_ns)
            }
        }
    }

    /// Reports what the call showed. The venue's request limit is the venue's, not the path's, so a
    /// throttled answer to any call holds reads, including one to a cancel.
    pub fn report(&mut self, class: CallClass, now_ns: i128, result: CallResult) {
        if let CallResult::Throttled { retry_after_ns: Some(wait_ns) } = result {
            self.reads.retry_after(now_ns, wait_ns);
        }
        let breaker = match class {
            CallClass::Submit => &mut self.submit,
            CallClass::Read => &mut self.read,
            CallClass::Cancel => return,
        };
        match result {
            CallResult::Unhealthy => breaker.failed(now_ns),
            CallResult::Healthy | CallResult::Throttled { .. } => breaker.succeeded(now_ns),
        }
    }
}

fn permit(breaker: &mut CircuitBreaker, now_ns: i128) -> Result<(), Held> {
    match breaker.permit(now_ns) {
        Permit::Go | Permit::Probe => Ok(()),
        Permit::Refused { retry_at_ns } => Err(Held::CircuitOpen { retry_at_ns }),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resilience/gate_tests.rs"]
mod tests;
