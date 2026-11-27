// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A projection that cannot be read before it has been rebuilt.
//! `docs/concepts/event_sourcing.md` carries why the guard is the type rather than the caller.

use std::path::Path;

use crate::store::{StoreError, read_range};

/// How far a rebuild got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Recovery {
    /// The replay has not completed. Nothing may be read. The default, so an unlooked projection is never trusted.
    #[default]
    Incomplete,
    /// Rebuilt over the whole stream, through this position.
    /// `through` is `None` for an empty stream: a verified empty history, not an unlooked one.
    Verified { through: Option<u64> },
}

impl Recovery {
    pub fn is_verified(self) -> bool {
        matches!(self, Recovery::Verified { .. })
    }
}

/// Something rebuilt by folding events in order.
/// Decoding belongs to the projection: it knows its own class and codec, the core knows neither.
pub trait Projection: Default {
    type Event;
    type Error;

    fn decode(payload: &[u8]) -> Result<Self::Event, Self::Error>;

    /// Fold one event in. Refusing here stops the whole rebuild.
    fn apply(&mut self, event: &Self::Event) -> Result<(), Self::Error>;
}

#[derive(Debug)]
pub enum RebuildError<E> {
    Store(StoreError),
    /// An event this build could not read. Refused, never skipped:
    /// a projection that skipped one would be confidently wrong.
    Undecodable {
        at: u64,
        cause: E,
    },
    Rejected {
        at: u64,
        cause: E,
    },
}

impl<E: core::fmt::Debug> core::fmt::Display for RebuildError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "store: {e:?}"),
            Self::Undecodable { at, cause } => {
                write!(f, "the event at position {at} could not be read: {cause:?}")
            }
            Self::Rejected { at, cause } => {
                write!(f, "the event at position {at} was refused: {cause:?}")
            }
        }
    }
}

impl<E: core::fmt::Debug> std::error::Error for RebuildError<E> {}

/// A projection and the evidence that it was rebuilt. They travel together so one cannot be read without the other.
#[derive(Debug, Default)]
pub struct Recovered<P> {
    inner: P,
    recovery: Recovery,
}

impl<P: Projection> Recovered<P> {
    /// Rebuild from the whole stream. No partial success. The value is verified over the whole history or does not exist.
    pub fn rebuild(root: &Path, stream: &str) -> Result<Self, RebuildError<P::Error>> {
        let entries = read_range(root, stream, 0, u64::MAX).map_err(RebuildError::Store)?;
        let mut inner = P::default();
        let mut through = None;

        for entry in entries {
            let event = P::decode(&entry.payload)
                .map_err(|cause| RebuildError::Undecodable { at: entry.seq, cause })?;
            inner.apply(&event).map_err(|cause| RebuildError::Rejected { at: entry.seq, cause })?;
            through = Some(entry.seq);
        }

        Ok(Self { inner, recovery: Recovery::Verified { through } })
    }

    /// The projection, or `None` until the rebuild has completed. The type holds the guard, not each method.
    pub fn state(&self) -> Option<&P> {
        self.recovery.is_verified().then_some(&self.inner)
    }

    pub fn recovery(&self) -> Recovery {
        self.recovery
    }

    /// Advance a rebuilt projection with an event consumed after the rebuild.
    /// Refuses before recovery: state would look current and be founded on nothing.
    pub fn advance(&mut self, event: &P::Event) -> Result<(), AdvanceError<P::Error>> {
        if !self.recovery.is_verified() {
            return Err(AdvanceError::NotRecovered);
        }
        self.inner.apply(event).map_err(AdvanceError::Rejected)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AdvanceError<E> {
    NotRecovered,
    Rejected(E),
}

#[cfg(test)]
#[path = "../../tests/unit/projection/tests.rs"]
mod tests;
