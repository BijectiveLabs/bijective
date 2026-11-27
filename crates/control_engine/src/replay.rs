// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Every fold the control stream feeds, taken in a single read.
//! The stream grows with every order, so it is read once per start.

use bijective_event_store::projection::Projection;

use crate::daily::{Daily, DailyError};
use crate::linked::{Linked, LinkedError};
use crate::orders::{Orders, OrdersError};
use crate::progress::{Progress, ProgressError};
use crate::unverified::{Unverified, UnverifiedError};
use crate::wire::{ControlEvent, decode};

#[derive(Debug)]
pub enum ReplayError {
    Decode(bijective_serialization::codec::DecodeError),
    Progress(ProgressError),
    Unverified(UnverifiedError),
    Orders(OrdersError),
    Daily(DailyError),
    Linked(LinkedError),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Decode(e) => write!(f, "decode: {e}"),
            ReplayError::Progress(e) => write!(f, "progress: {e}"),
            ReplayError::Unverified(e) => write!(f, "unverified: {e}"),
            ReplayError::Orders(e) => write!(f, "orders: {e}"),
            ReplayError::Daily(e) => write!(f, "daily: {e}"),
            ReplayError::Linked(e) => write!(f, "linked: {e}"),
        }
    }
}

impl std::error::Error for ReplayError {}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Replayed {
    progress: Progress,
    unverified: Unverified,
    orders: Orders,
    daily: Daily,
    linked: Linked,
}

impl Replayed {
    #[must_use]
    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    #[must_use]
    pub fn unverified(&self) -> &Unverified {
        &self.unverified
    }

    #[must_use]
    pub fn orders(&self) -> &Orders {
        &self.orders
    }

    #[must_use]
    pub fn daily(&self) -> &Daily {
        &self.daily
    }

    #[must_use]
    pub fn linked(&self) -> &Linked {
        &self.linked
    }
}

impl Projection for Replayed {
    type Event = ControlEvent;
    type Error = ReplayError;

    fn decode(payload: &[u8]) -> Result<ControlEvent, ReplayError> {
        decode(payload).map_err(ReplayError::Decode)
    }

    fn apply(&mut self, event: &ControlEvent) -> Result<(), ReplayError> {
        // All always run. One fold without another would leave them disagreeing on the same history.
        self.progress.apply(event).map_err(ReplayError::Progress)?;
        self.unverified.apply(event).map_err(ReplayError::Unverified)?;
        self.orders.apply(event).map_err(ReplayError::Orders)?;
        self.daily.apply(event).map_err(ReplayError::Daily)?;
        self.linked.apply(event).map_err(ReplayError::Linked)?;
        Ok(())
    }
}
