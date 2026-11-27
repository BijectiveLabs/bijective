// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Time, read through one interface.
//! The platform architecture. `docs/concepts/event_sourcing.md` says why the wall clock follows the host but never goes back.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Nanoseconds since the Unix epoch. `i128` to match money and exposure, signed so a duration has a direction.
pub type Nanos = i128;

pub trait Clock: Send + Sync {
    fn now_ns(&self) -> Nanos;
}

/// A reading as a person reads it: `2026-09-04 14:31:02Z`, UTC only.
/// One shared copy of the arithmetic, so surfaces cannot disagree on what a timestamp means.
#[must_use]
pub fn as_utc(ns: Nanos) -> String {
    let seconds = ns.div_euclid(1_000_000_000);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3_600, (rest % 3_600) / 60, rest % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

/// Days since the epoch to a civil date, by the shift-the-era method.
fn civil_from_days(days: i128) -> (i128, i128, i128) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Wall time that never reads earlier than it did and never falls behind the host.
/// Each reading is the later of the operating system's clock and the last reading: a step back (an
/// NTP correction) holds time still until the host catches up, and a suspended host, which a
/// monotonic clock does not count, is caught up on the first reading after it resumes.
/// The only read of the operating system's clock in the platform.
pub struct WallClock {
    last_ns: AtomicI64,
}

impl WallClock {
    pub fn new() -> Self {
        Self { last_ns: AtomicI64::new(i64::MIN) }
    }
}

impl Default for WallClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for WallClock {
    fn now_ns(&self) -> Nanos {
        later_of(&self.last_ns, system_ns())
    }
}

fn system_ns() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// The reading for a host reading of `host_ns`: never earlier than any reading already given.
fn later_of(last_ns: &AtomicI64, host_ns: i64) -> Nanos {
    last_ns.fetch_max(host_ns, Ordering::SeqCst).max(host_ns) as Nanos
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockError {
    /// An attempt to move a sequence clock backwards.
    /// Refused, not ignored: a silent skip would diverge from the log while reporting success.
    WouldGoBackwards { at_ns: Nanos, asked_ns: Nanos },
}

impl core::fmt::Display for ClockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WouldGoBackwards { at_ns, asked_ns } => {
                write!(f, "the clock reads {at_ns} and was asked to move to {asked_ns}")
            }
        }
    }
}

impl std::error::Error for ClockError {}

/// Time driven by an event sequence. Reads nothing from the host.
/// Same sequence, same readings: what byte-identical replay rests on.
pub struct SequenceClock {
    at_ns: AtomicI64,
}

impl SequenceClock {
    pub fn starting_at(at_ns: Nanos) -> Self {
        Self { at_ns: AtomicI64::new(at_ns as i64) }
    }

    /// Move time to an event's timestamp. Equal is permitted. Earlier is refused.
    pub fn advance_to(&self, at_ns: Nanos) -> Result<(), ClockError> {
        let current = self.now_ns();
        if at_ns < current {
            return Err(ClockError::WouldGoBackwards { at_ns: current, asked_ns: at_ns });
        }
        self.at_ns.store(at_ns as i64, Ordering::SeqCst);
        Ok(())
    }
}

impl Clock for SequenceClock {
    fn now_ns(&self) -> Nanos {
        self.at_ns.load(Ordering::SeqCst) as Nanos
    }
}

#[cfg(test)]
#[path = "../../tests/unit/clock/tests.rs"]
mod tests;
