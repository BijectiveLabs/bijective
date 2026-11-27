// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Subscription acknowledgement - the confirmed-subscription rule made structural.

/// The confirmed-subscription rule: a subscription not confirmed attached is not subscribed. Separate variants force callers to handle both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionAck {
    /// The venue confirmed the subscription is attached.
    Confirmed { at_ns: i128 },
    /// Not subscribed: sending is not attaching.
    Unconfirmed { requested_at_ns: i128 },
}

impl SubscriptionAck {
    /// Whether the platform may rely on receiving this data.
    pub fn is_attached(&self) -> bool {
        matches!(self, SubscriptionAck::Confirmed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeError {
    /// Not served per the declaration. Refused before the request is sent.
    NotServed,
    /// The venue refused the subscription.
    Refused { venue_code: String },
    /// The connection is not in a state that can carry a subscription.
    NotConnected,
}

impl core::fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SubscribeError::NotServed => write!(f, "the venue does not serve this data class"),
            SubscribeError::Refused { venue_code } => {
                write!(f, "the venue refused the subscription: {venue_code}")
            }
            SubscribeError::NotConnected => write!(f, "not connected"),
        }
    }
}

impl std::error::Error for SubscribeError {}

impl core::fmt::Display for SubscriptionAck {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SubscriptionAck::Confirmed { at_ns } => write!(f, "confirmed at {at_ns}ns"),
            SubscriptionAck::Unconfirmed { requested_at_ns } => {
                write!(f, "unconfirmed, asked for at {requested_at_ns}ns")
            }
        }
    }
}
