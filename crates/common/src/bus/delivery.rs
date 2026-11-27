// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a publication did, stated rather than implied.

use crate::bus::SubscriptionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub topic: String,
    pub payload: Vec<u8>,
    /// Position within the topic.
    pub sequence: u64,
    /// Read through the clock, never the system clock.
    pub stamped_at_ns: i128,
}

/// One subscriber that could not take a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub subscription: SubscriptionId,
    /// How many it was already holding: operators need how far behind.
    pub behind_by: usize,
}

/// What one publication did. Returned, not logged, so ignoring a refusal is deliberate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub topic: String,
    pub sequence: u64,
    pub stamped_at_ns: i128,
    pub delivered: usize,
    pub refused: Vec<Refusal>,
}

impl Publication {
    pub fn reached_everyone(&self) -> bool {
        self.refused.is_empty()
    }

    /// Whether a subscriber took it. A publication with no subscriber differs from one a subscriber missed.
    pub fn reached_anyone(&self) -> bool {
        self.delivered > 0
    }
}
