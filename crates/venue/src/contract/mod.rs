// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The venue adapter contract. Design notes: `docs/concepts/adapters.md`.

pub mod conformance;
pub mod connection;
pub mod record;
pub mod subscription;

pub use connection::{ConnectionState, Continuity, DegradedReason, Moment, Reconnected};
pub use record::{RecordDefect, check_record, class_of};
pub use subscription::{SubscribeError, SubscriptionAck};

use crate::normalise::VenueOutcome;

// Re-exported so implementing a trait needs no second dependency.
pub use crate::profile::{CapabilityDeclaration, DataClass};

// Platform objects live in the core. This crate defines only the vocabulary of crossing the boundary.
pub use bijective_model::market;
pub use bijective_model::market::{MarketFact, MarketRecord};
pub use bijective_model::{
    AccountMargin, AccountSnapshot, BookTop, Clearance, InstrumentId, OrderRequest,
    PositionSnapshot, Sequence, Side, VenueSymbol,
};
pub use bijective_model::{OrderTerms, TimeInForce};

use std::sync::Arc;

use bijective_common::clock::Clock;

/// Boxed and pinned for object safety: one suite must run against `&mut dyn VenueData`.
pub type Answer<'a, T> = core::pin::Pin<Box<dyn core::future::Future<Output = T> + Send + 'a>>;

/// Market-data side. Separate from execution so a read-only key can serve data.
pub trait VenueData: Send {
    /// Derived from the venue profile. On the contract so an adapter cannot be used without it.
    fn declaration(&self) -> &CapabilityDeclaration;

    fn connect(&mut self, now_ns: i128) -> Answer<'_, Result<ConnectionState, ConnectError>>;

    fn disconnect(&mut self) -> Answer<'_, ()>;

    /// The last state observed. An accessor, so it reaches nothing and stays synchronous.
    fn state(&self) -> ConnectionState;

    /// Reconnect, reporting whether the stream resumed or crossed a gap.
    fn reconnect(&mut self, now_ns: i128) -> Answer<'_, Result<Reconnected, ConnectError>>;

    fn subscribe<'a>(
        &'a mut self,
        class: DataClass,
        instrument: &'a VenueSymbol,
        now_ns: i128,
    ) -> Answer<'a, Result<SubscriptionAck, SubscribeError>>;
}

/// The execution side.
pub trait VenueExecution: Send {
    fn declaration(&self) -> &CapabilityDeclaration;

    /// The clearance exists only from a cleared risk evaluation. The outcome is not a `Result` because "could not tell" is an answer.
    fn submit<'a>(
        &'a mut self,
        clearance: &'a Clearance,
        order: &'a OrderRequest,
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome>;

    /// No clearance, deliberately: a cancel only reduces exposure, so a platform in breach must still exit.
    /// The instrument travels with the identifier because a venue keys a cancel by both.
    fn cancel<'a>(
        &'a mut self,
        instrument: &'a VenueSymbol,
        client_id: &'a str,
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome>;

    /// The venue's own account, for reconciliation to match against ours.
    fn account(&mut self, now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>>;

    /// What the venue's push channel said since the last ask. Asking clears a reported change.
    /// The default keeps no channel. An accessor, so it reaches nothing and stays synchronous.
    fn pushed(&mut self) -> Pushed {
        Pushed::NotKept
    }

    /// The orders a previous process placed on this venue, from the record, handed over once when
    /// the venue opens and before its evidence is read. A venue that carries the client
    /// identifier needs none of it, which is the default. One that knows an order only by the
    /// hash of its signed content derives that hash again, so its evidence names the order as
    /// this platform's after a restart. Local state only: it reaches nothing.
    fn adopt(&mut self, _recorded: &[Recorded]) {}

    /// What the venue holds, as evidence for reconciliation: its orders, its fills since
    /// `since_ns` and its positions, with what the read covered. The default says this adapter
    /// publishes none, which reconciliation reads as unverified, never as aligned.
    fn evidence(
        &mut self,
        _since_ns: i128,
        _now_ns: i128,
    ) -> Answer<'_, Result<VenueEvidence, ConnectError>> {
        Box::pin(async {
            Err(ConnectError::NotPermitted {
                detail: "this adapter publishes no venue evidence".into(),
            })
        })
    }
}

pub use bijective_lifecycle::VenueEvidence;

/// An order a previous process placed on a venue, as the platform's record holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub request: OrderRequest,
    /// When its intent was recorded: the instant its submit was made at.
    pub intent_ns: i128,
    /// The venue's own identifier, where its evidence has named one.
    pub venue_order_id: Option<String>,
    /// The record holds the order finished: the venue will change nothing of it.
    pub terminal: bool,
}

/// What a venue's push channel (a private order and execution stream) said since the last ask.
/// The channel only prompts a read: venue evidence stays the one source of order states and
/// fills, so a lost or late push costs latency, never correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    /// This adapter keeps no push channel. The reconciliation cadence is the only reader.
    NotKept,
    /// Attached, and the venue reported nothing since the last ask.
    Quiet,
    /// The venue reported an order or execution, or the channel attached after a gap that could
    /// hide one: what the venue holds should be read now.
    Changed,
    /// Not attached. Changes are found on the reconciliation cadence until it returns.
    Lapsed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The venue is not registered in this build, or is curated for market data only.
    NotPermitted {
        detail: String,
    },
    /// No credential was resolvable for this side.
    NoCredential,
    Transport {
        detail: String,
    },
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConnectError::NotPermitted { detail } => write!(f, "not permitted: {detail}"),
            ConnectError::NoCredential => write!(f, "no credential is resolvable for this side"),
            ConnectError::Transport { detail } => write!(f, "transport: {detail}"),
        }
    }
}

impl std::error::Error for ConnectError {}

#[cfg(test)]
#[path = "../../tests/unit/contract/tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    /// A frame that could not be read. Refused, never dropped.
    Unreadable { detail: String },
    /// The venue's sequence broke for an instrument whose book we hold.
    SequenceBroken { instrument: String },
    /// An incremental update arrived for an instrument that has had no snapshot.
    NoSnapshot { instrument: String },
    /// A record the adapter built broke the record contract. Refused before it left the adapter.
    RecordRefused(RecordDefect),
}

impl core::fmt::Display for FeedError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unreadable { detail } => write!(f, "frame refused: {detail}"),
            Self::SequenceBroken { instrument } => write!(f, "sequence broken on {instrument}"),
            Self::NoSnapshot { instrument } => write!(f, "update before snapshot on {instrument}"),
            Self::RecordRefused(defect) => write!(f, "record refused: {defect}"),
        }
    }
}

impl std::error::Error for FeedError {}

/// What one frame became. Both empty means understood with nothing to report (an ack, a
/// heartbeat), never discarded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offered {
    /// Every fact the frame carried, in the order the venue wrote them, each passing
    /// [`check_record`].
    pub records: Vec<MarketRecord>,
    /// The top of the book the frame left, where it touched a book with both sides.
    pub top: Option<BookTop>,
}

/// Turns wire frames, as bytes, into normalised observations without the caller knowing the venue.
pub trait MarketFeed {
    /// `received_at_ns` is the arrival instant on the platform clock, or the recorded one on
    /// replay. It is stamped onto every record the frame yields.
    fn offer(&mut self, frame: &[u8], received_at_ns: i128) -> Result<Offered, FeedError>;

    /// Forget every book on a continuity break. A carried book is quietly wrong.
    fn reset(&mut self);

    fn accepted(&self) -> u64;

    fn instruments(&self) -> usize;
}

/// What a live stream hands upward. Parsed once in the adapter so no caller needs the wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketEvent {
    /// The frame exactly as it arrived, sent before anything parsed from it, so a consumer can
    /// make it durable before acting on what it said. Opaque above the adapter: carried and
    /// stored, never interpreted.
    Received(ReceivedFrame),
    /// Sent after the records of the same frame, so a consumer of both sees the cause first.
    Observation(BookTop),
    Record(MarketRecord),
    /// Reported on every transition, never inferred from the absence of messages.
    State(ConnectionState),
    /// A frame that could not become an observation. Carried, not dropped.
    Refused(FeedError),
    /// Continuity broke and books were dropped. A consumer that misses it holds pre-gap data.
    ContinuityLost,
}

/// One frame as the socket delivered it, with its arrival on the platform clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFrame {
    pub received_at_ns: i128,
    pub frame: Vec<u8>,
}

/// What a live market stream is asked to carry. The clock stamps arrival. It is injected so a
/// replayed run and a live one take time from the same interface.
#[derive(Clone)]
pub struct StreamRequest {
    pub endpoint: String,
    pub instruments: Vec<String>,
    /// Each instrument's scales, from the profile (decision 0007).
    pub scales: crate::profile::InstrumentScales,
    pub clock: Arc<dyn Clock>,
}

/// An adapter that stopped rather than answered. Neither a refusal nor an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panicked;

/// Contains one adapter panic. The caller must withdraw that venue and never call the adapter again.
pub fn contain<T>(work: Answer<'_, T>) -> Contained<'_, T> {
    Contained { work }
}

pub struct Contained<'a, T> {
    work: Answer<'a, T>,
}

impl<T> core::future::Future for Contained<'_, T> {
    type Output = Result<T, Panicked>;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        use core::task::Poll;

        // `Answer` is a pinned box, so it is `Unpin` and this needs no unsafe projection.
        let work = &mut self.get_mut().work;
        let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            core::future::Future::poll(work.as_mut(), cx)
        }));

        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(_) => Poll::Ready(Err(Panicked)),
        }
    }
}
