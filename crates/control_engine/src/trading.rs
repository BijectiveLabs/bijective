// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The one trading state. See `docs/concepts/order_path.md`.

use bijective_limits::{SafeState, TradingStateControl};

/// Severity rank. Not `Ord`: a derived order would change meaning if a state were inserted.
const fn severity(state: SafeState) -> u8 {
    match state {
        SafeState::Active => 0,
        SafeState::Reducing => 1,
        SafeState::Halted => 2,
    }
}

#[derive(Debug)]
pub struct TradingState {
    current: SafeState,
}

impl Default for TradingState {
    /// Halted: a fresh process knows nothing about the world and must not trade.
    fn default() -> Self {
        Self { current: SafeState::Halted }
    }
}

impl TradingState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn current(&self) -> SafeState {
        self.current
    }

    #[must_use]
    pub fn admits_new_orders(&self) -> bool {
        matches!(self.current, SafeState::Active)
    }

    /// Return to a less restrictive state. Not reachable through the control port: only a judgement
    /// that the cause is gone may do it, and no automatic path holds that.
    pub fn resume(&mut self, state: SafeState) {
        self.current = state;
    }
}

impl TradingStateControl for TradingState {
    /// Escalate to `state`, or stay. A less restrictive request is ignored so arrival order
    /// cannot reopen the platform (halt, then a scoped Reducing).
    fn set(&mut self, state: SafeState) {
        if severity(state) > severity(self.current) {
            self.current = state;
        }
    }

    fn current(&self) -> SafeState {
        self.current
    }
}
