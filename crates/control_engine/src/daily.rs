// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The unrealised profit the book held when a UTC day opened, rebuilt from the control stream.
//!
//! A daily loss is the day's result measured from this figure. Holding it in the stream, not in
//! memory, keeps a restart in the middle of a day from measuring from a later, smaller baseline.

use bijective_event_store::projection::Projection;

use crate::wire::{ControlEvent, decode};

#[derive(Debug)]
pub enum DailyError {
    Decode(bijective_serialization::codec::DecodeError),
}

impl std::fmt::Display for DailyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DailyError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

impl std::error::Error for DailyError {}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Daily {
    /// The latest day opened and its baseline, at the money scale.
    opened: Option<(u64, i128)>,
    /// The latest day with a recorded peak and that peak, at the money scale.
    peak: Option<(u64, i128)>,
}

impl Daily {
    /// The baseline of `day` (days since the epoch), or `None` when that day has not opened.
    #[must_use]
    pub fn baseline_for(&self, day: u64) -> Option<i128> {
        self.opened.filter(|(opened, _)| *opened == day).map(|(_, baseline)| baseline)
    }

    /// The highest result recorded for `day`, or `None` when none was recorded.
    #[must_use]
    pub fn peak_for(&self, day: u64) -> Option<i128> {
        self.peak.filter(|(peaked, _)| *peaked == day).map(|(_, peak)| peak)
    }
}

impl Projection for Daily {
    type Event = ControlEvent;
    type Error = DailyError;

    fn decode(payload: &[u8]) -> Result<ControlEvent, DailyError> {
        decode(payload).map_err(DailyError::Decode)
    }

    /// The first record of a day stands: a second would let the loss so far be forgotten.
    fn apply(&mut self, event: &ControlEvent) -> Result<(), DailyError> {
        match event {
            ControlEvent::DayOpened { day, unrealized_scaled, .. } => {
                let already = self.opened.is_some_and(|(opened, _)| opened >= *day);
                if !already {
                    self.opened = Some((*day, *unrealized_scaled));
                }
            }
            // A peak only rises within its day, so a lower or older record changes nothing.
            ControlEvent::DayPeak { day, result_scaled, .. } => {
                let higher = match self.peak {
                    Some((peaked, peak)) => {
                        *day > peaked || (*day == peaked && *result_scaled > peak)
                    }
                    None => true,
                };
                if higher {
                    self.peak = Some((*day, *result_scaled));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/daily_tests.rs"]
mod tests;
