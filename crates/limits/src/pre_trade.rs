// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The pre-trade check: same-instant judgement over held state, linear in declared limits.

use super::*;

/// An action, not a report: `Copy`, so nothing allocates on the path the order waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreTradeVerdict {
    Continue,
    Drive { state: SafeState, reason: SafeStateReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldState {
    pub(crate) snapshot: ExposureSnapshot,
    pub(crate) position_held: PositionHeld,
    pub(crate) received_at_ns: i128,
}

/// Same-instant half of risk limits. Fail-closed: no policy input within the bound is evidence that nothing is being checked.
/// No I/O, lock or allocation on the assessment path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreTradeCheck {
    pub(crate) limits: Vec<Limit>,
    pub(crate) staleness_bound_ns: i128,
    pub(crate) semantics: ReduceOnlySemantics,
    pub(crate) held: Option<HeldState>,
}

impl PreTradeCheck {
    /// Allocation happens here, not on the assessment path.
    #[must_use]
    pub fn new(
        limits: Vec<Limit>,
        staleness_bound_ns: i128,
        semantics: ReduceOnlySemantics,
    ) -> Self {
        Self { limits, staleness_bound_ns, semantics, held: None }
    }

    /// `received_at_ns` is arrival time, not observation time: a stalled producer with a drifting
    /// clock must not look like it is still speaking.
    pub fn on_snapshot(
        &mut self,
        snapshot: ExposureSnapshot,
        position_held: PositionHeld,
        received_at_ns: i128,
    ) {
        self.held = Some(HeldState { snapshot, position_held, received_at_ns });
    }

    /// Strictest uncertainty first: never primed, stale inputs, unjudgeable limit, then breach. The order
    /// is a safety decision (`docs/concepts/risk_and_limits.md`). Returns on the first finding.
    #[must_use]
    pub fn assess(&self, now_ns: i128) -> PreTradeVerdict {
        let Some(held) = &self.held else {
            return PreTradeVerdict::Drive {
                state: SafeState::Halted,
                reason: SafeStateReason::NeverPrimed,
            };
        };

        let stale_for_ns = now_ns - held.received_at_ns;
        if stale_for_ns > self.staleness_bound_ns {
            return PreTradeVerdict::Drive {
                state: SafeState::Halted,
                reason: SafeStateReason::InputsStale { stale_for_ns },
            };
        }

        for limit in &self.limits {
            let Some((_, _, observed)) = held
                .snapshot
                .figures
                .iter()
                .find(|(scope, kind, _)| *scope == *limit.scope() && *kind == limit.kind)
            else {
                return PreTradeVerdict::Drive {
                    state: SafeState::Halted,
                    reason: SafeStateReason::Unjudgeable { kind: limit.kind },
                };
            };

            match limit.judge(*observed) {
                Ok(Some(breach)) => {
                    return PreTradeVerdict::Drive {
                        state: select_safe_state(&breach, held.position_held, self.semantics),
                        reason: SafeStateReason::Breached(breach),
                    };
                }
                Ok(None) => {}
                // A figure it cannot compare is a figure it does not have: halt, as for a missing one.
                Err(LimitError::UnitMismatch { kind, .. } | LimitError::Unreachable { kind }) => {
                    return PreTradeVerdict::Drive {
                        state: SafeState::Halted,
                        reason: SafeStateReason::Unjudgeable { kind },
                    };
                }
            }
        }

        PreTradeVerdict::Continue
    }
}
