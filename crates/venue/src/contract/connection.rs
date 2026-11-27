// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Connection state, and the gap a reconnect must never hide.

/// Reported accurately including degraded states. Two states would let an attached-but-useless link read healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected {
        since: Moment,
    },
    /// Attached, and something the caller must know about is wrong.
    Degraded {
        since: Moment,
        reason: DegradedReason,
    },
}

/// Not a zero and not an `Option`: zero reads as the epoch and `None` is one `unwrap_or(0)` away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    Observed(i128),
    /// This adapter does not keep the figure. Distinct from *the figure is zero*.
    NotTracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DegradedReason {
    /// Messages are arriving and the consumer is behind.
    Backlogged { queued: usize },
    /// Nothing has arrived for longer than the profile's staleness bound.
    Stale { last_message: Moment },
    /// Some subscriptions attached and some did not.
    SubscriptionsIncomplete { attached: usize, requested: usize },
}

impl ConnectionState {
    /// `Degraded` is false: attached and stale is worse than down, because stale looks like calm.
    pub fn is_usable(&self) -> bool {
        matches!(self, ConnectionState::Connected { .. })
    }
}

/// Whether a reconnect crossed a gap. An unknown extent is the worse fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuity {
    Unbroken,
    Gap {
        from_seq: u64,
        to_seq: u64,
    },
    /// A gap occurred and its extent could not be established.
    GapOfUnknownExtent,
}

impl core::fmt::Display for Continuity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Continuity::Unbroken => write!(f, "unbroken"),
            // Naming both ends separates a gap that can be re-fetched from one that can only be declared.
            Continuity::Gap { from_seq, to_seq } => {
                write!(f, "a gap from {from_seq} to {to_seq}")
            }
            Continuity::GapOfUnknownExtent => write!(f, "a gap of unknown extent"),
        }
    }
}

impl Continuity {
    /// Whether anything downstream may treat the stream as continuous.
    pub fn is_unbroken(&self) -> bool {
        matches!(self, Continuity::Unbroken)
    }

    /// A bounded gap can be re-fetched or declared. An unbounded one only declared.
    pub fn extent_is_known(&self) -> bool {
        !matches!(self, Continuity::GapOfUnknownExtent)
    }
}

impl core::fmt::Display for Moment {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // Not tracked is not zero. The type is a sum rather than a sentinel integer.
            Moment::Observed(at_ns) => write!(f, "{at_ns}ns"),
            Moment::NotTracked => write!(f, "not tracked by this adapter"),
        }
    }
}

impl core::fmt::Display for DegradedReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DegradedReason::Backlogged { queued } => write!(f, "{queued} message(s) queued"),
            DegradedReason::Stale { last_message } => {
                write!(f, "nothing since {last_message}")
            }
            DegradedReason::SubscriptionsIncomplete { attached, requested } => {
                write!(f, "{attached} of {requested} subscriptions attached")
            }
        }
    }
}

impl core::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "disconnected"),
            ConnectionState::Connecting => write!(f, "connecting"),
            ConnectionState::Connected { since } => write!(f, "connected since {since}"),
            ConnectionState::Degraded { since, reason } => {
                write!(f, "degraded since {since}: {reason}")
            }
        }
    }
}

/// Continuity travels with the state so a caller cannot take the connection and forget to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconnected {
    pub state: ConnectionState,
    pub continuity: Continuity,
}
