// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! When to stop calling a venue that keeps failing, and when to try it again.
//!
//! Closed: calls go through and consecutive failures are counted. Open: calls are refused without
//! being made, until a period has passed. Half open: one probe call is let through. Its result
//! closes the breaker or opens it for another period. An open breaker therefore always comes back
//! to try. It cannot keep a recovered venue shut out.

/// What opens the breaker and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerConfig {
    /// Consecutive failures that open it. At least one.
    pub failures_to_open: u32,
    /// How long it stays open, and how long a probe may stay unreported before another is allowed.
    pub open_for_ns: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerError {
    NoThreshold,
    NoOpenPeriod,
}

impl std::fmt::Display for BreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoThreshold => write!(f, "a breaker needs at least one failure to open on"),
            Self::NoOpenPeriod => write!(f, "a breaker needs an open period above zero"),
        }
    }
}

impl std::error::Error for BreakerError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open { until_ns: i128 },
    HalfOpen,
}

/// The answer to "may I call?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permit {
    /// Call. The breaker is closed.
    Go,
    /// Call, as the single probe. Report its result with `succeeded` or `failed`.
    Probe,
    /// Do not call. Ask again at `retry_at_ns`.
    Refused { retry_at_ns: i128 },
}

/// Time is the caller's: every method takes `now_ns`, so the breaker is a function of its inputs.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    config: BreakerConfig,
    state: CircuitState,
    consecutive_failures: u32,
    probe_started_ns: Option<i128>,
}

impl CircuitBreaker {
    pub fn new(config: BreakerConfig) -> Result<Self, BreakerError> {
        if config.failures_to_open == 0 {
            return Err(BreakerError::NoThreshold);
        }
        if config.open_for_ns <= 0 {
            return Err(BreakerError::NoOpenPeriod);
        }
        Ok(Self {
            config,
            state: CircuitState::Closed,
            consecutive_failures: 0,
            probe_started_ns: None,
        })
    }

    #[must_use]
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Asks to make a call. A `Go` or `Probe` must be followed by `succeeded` or `failed`.
    pub fn permit(&mut self, now_ns: i128) -> Permit {
        match self.state {
            CircuitState::Closed => Permit::Go,
            CircuitState::Open { until_ns } if now_ns < until_ns => {
                Permit::Refused { retry_at_ns: until_ns }
            }
            CircuitState::Open { .. } => {
                self.state = CircuitState::HalfOpen;
                self.probe_started_ns = Some(now_ns);
                Permit::Probe
            }
            CircuitState::HalfOpen => {
                let started = self.probe_started_ns.unwrap_or(now_ns);
                if now_ns - started >= self.config.open_for_ns {
                    // The earlier probe never reported. Waiting for it forever would wedge the breaker.
                    self.probe_started_ns = Some(now_ns);
                    Permit::Probe
                } else {
                    Permit::Refused { retry_at_ns: started + self.config.open_for_ns }
                }
            }
        }
    }

    /// A permitted call completed. A result that arrives while open is stale (the call began
    /// before the breaker opened) and changes nothing.
    pub fn succeeded(&mut self, _now_ns: i128) {
        match self.state {
            CircuitState::Closed => self.consecutive_failures = 0,
            CircuitState::HalfOpen => {
                self.state = CircuitState::Closed;
                self.consecutive_failures = 0;
                self.probe_started_ns = None;
            }
            CircuitState::Open { .. } => {}
        }
    }

    /// A permitted call failed in a way that says the venue is unhealthy: a timeout, a lost
    /// connection, a server error. A refusal of the request itself is not a failure.
    pub fn failed(&mut self, now_ns: i128) {
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= self.config.failures_to_open {
                    self.open(now_ns);
                }
            }
            CircuitState::HalfOpen => self.open(now_ns),
            CircuitState::Open { .. } => {}
        }
    }

    fn open(&mut self, now_ns: i128) {
        self.state =
            CircuitState::Open { until_ns: now_ns.saturating_add(self.config.open_for_ns) };
        self.probe_started_ns = None;
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resilience/breaker_tests.rs"]
mod tests;
