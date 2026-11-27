// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Connection session as a state machine that touches nothing, so failure paths run in tests.
//! One subscription frame names every outcome. Each outcome's book is asked for again when none
//! has arrived for twenty seconds, which also repairs a book the feed dropped for disagreeing
//! with the venue's stated best prices.

use std::collections::BTreeMap;

use bijective_venue::contract::{ConnectionState, DegradedReason, Moment};
use bijective_venue::resilience::{Backoff, Jitter};

use crate::message::{PING_INTERVAL_NS, VenueEvent};

/// The venue answers each `PING` with `PONG` every ten seconds. Three unanswered mean the link is dead.
pub const LINK_SILENT_NS: i128 = 30_000_000_000;

/// A subscription is confirmed only by data. A quiet stream is asked again rather than
/// reconnecting every stream.
pub const STREAM_SILENT_NS: i128 = 20_000_000_000;

/// Resubscribe attempts before the socket itself is suspected.
pub const RESUBSCRIBE_LIMIT: u32 = 3;

/// A quarter of a second doubling to five seconds. Checked when the crate is built.
const RECONNECT: Backoff = match Backoff::new(250_000_000, 5_000_000_000) {
    Ok(backoff) => backoff,
    Err(_) => panic!("the reconnect backoff constants are valid"),
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<'a> {
    Tick,
    Connected,
    ConnectFailed,
    SocketClosed,
    Inbound(&'a [VenueEvent]),
    /// The feed holds no usable book for this outcome. Ask for one now.
    Resnapshot(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Connect,
    Send(String),
    Disconnect,
    /// A reconnect always crosses a gap of unknown extent.
    ResetBooks,
    Report(ConnectionState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    NoInstruments,
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoInstruments => write!(f, "a session was asked to carry no instruments"),
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug)]
struct Stream {
    seen: Option<i128>,
    asked_at: i128,
    attempts: u32,
}

#[derive(Debug)]
pub struct Session {
    jitter: Box<dyn Jitter>,
    /// Per instrument: last book, last request, request count. Idle time is measured from the
    /// request, not the link, or a busy socket hides a stream that never attached.
    streams: BTreeMap<String, Stream>,
    state: ConnectionState,
    attempts: u32,
    deadline_ns: Option<i128>,
    last_inbound_ns: i128,
    last_ping_ns: i128,
}

impl Session {
    /// Without jitter, so a test's deadlines are exact. Production builds the session with
    /// `with_jitter`. There is no other way to make one outside a test.
    #[cfg(test)]
    pub fn new(instruments: Vec<String>, now_ns: i128) -> Result<Self, SessionError> {
        Self::with_jitter(
            instruments,
            now_ns,
            Box::new(bijective_venue::resilience::Fixed(u64::MAX)),
        )
    }

    pub fn with_jitter(
        instruments: Vec<String>,
        now_ns: i128,
        jitter: Box<dyn Jitter>,
    ) -> Result<Self, SessionError> {
        if instruments.is_empty() {
            return Err(SessionError::NoInstruments);
        }

        Ok(Self {
            jitter,
            streams: instruments
                .into_iter()
                .map(|i| (i, Stream { seen: None, asked_at: now_ns, attempts: 0 }))
                .collect(),
            state: ConnectionState::Disconnected,
            attempts: 0,
            deadline_ns: Some(now_ns),
            last_inbound_ns: now_ns,
            last_ping_ns: now_ns,
        })
    }

    #[must_use]
    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    #[must_use]
    pub fn wake_at(&self) -> Option<i128> {
        self.deadline_ns
    }

    /// The opening frame: every outcome, with the lifecycle events (Real-time data).
    #[must_use]
    pub fn opening_frame(instruments: &[&String]) -> String {
        serde_json::json!({
            "assets_ids": instruments,
            "type": "market",
            "custom_feature_enabled": true
        })
        .to_string()
    }

    /// Asks for an outcome's book again on the open link. Subscribing an outcome already
    /// subscribed adds nothing and sends no book (`NO NEW ASSETS`, measured 2026-11-15), so the
    /// outcome is left and joined again, which sends its snapshot.
    #[must_use]
    pub fn rejoin_frames(instrument: &str) -> [String; 2] {
        ["unsubscribe", "subscribe"].map(|operation| {
            serde_json::json!({ "assets_ids": [instrument], "operation": operation }).to_string()
        })
    }

    #[must_use]
    pub fn ping_frame() -> String {
        "PING".to_string()
    }

    #[must_use]
    pub fn attached(&self) -> usize {
        self.streams.values().filter(|s| s.seen.is_some()).count()
    }

    pub fn step(&mut self, event: Event<'_>, now_ns: i128) -> Vec<Action> {
        let mut out = Vec::new();
        let before = self.state.clone();

        match event {
            Event::Tick => self.on_tick(now_ns, &mut out),
            Event::Connected => {
                self.attempts = 0;
                self.last_inbound_ns = now_ns;
                self.last_ping_ns = now_ns;
                // Nothing survives a reconnect on this venue.
                for stream in self.streams.values_mut() {
                    stream.seen = None;
                    stream.asked_at = now_ns;
                    stream.attempts = 0;
                }
                let every: Vec<&String> = self.streams.keys().collect();
                out.push(Action::Send(Self::opening_frame(&every)));
                self.state = ConnectionState::Degraded {
                    since: Moment::Observed(now_ns),
                    reason: DegradedReason::SubscriptionsIncomplete {
                        attached: 0,
                        requested: self.streams.len(),
                    },
                };
                self.deadline_ns = Some(now_ns + STREAM_SILENT_NS);
            }
            Event::ConnectFailed | Event::SocketClosed => self.retreat(now_ns),
            Event::Inbound(msg) => self.on_inbound(msg, now_ns),
            Event::Resnapshot(instrument) => self.resnapshot(instrument, now_ns, &mut out),
        }

        // Reported first so a consumer never sees a state following one it was not told about.
        if self.state != before {
            out.insert(0, Action::Report(self.state.clone()));
        }
        out
    }

    /// Asked once per outcome until its book arrives: every refused change after a break would
    /// otherwise ask again.
    fn resnapshot(&mut self, instrument: &str, now_ns: i128, out: &mut Vec<Action>) {
        if !matches!(
            self.state,
            ConnectionState::Connected { .. } | ConnectionState::Degraded { .. }
        ) {
            return;
        }
        let Some(stream) = self.streams.get_mut(instrument) else { return };
        if stream.seen.is_none() {
            return;
        }
        stream.seen = None;
        stream.asked_at = now_ns;
        out.extend(Self::rejoin_frames(instrument).map(Action::Send));
        self.settle(now_ns);
    }

    /// Only a book attaches an outcome: changes keep flowing to a book the feed has dropped.
    fn on_inbound(&mut self, events: &[VenueEvent], now_ns: i128) {
        self.last_inbound_ns = now_ns;
        let mut booked = false;
        for event in events {
            if let VenueEvent::Book(book) = event
                && let Some(stream) = self.streams.get_mut(&book.asset)
            {
                stream.seen = Some(now_ns);
                stream.attempts = 0;
                booked = true;
            }
        }
        if booked {
            self.settle(now_ns);
        }
    }

    /// A link already connected stays so: each book restating it would report a new state.
    fn settle(&mut self, now_ns: i128) {
        let attached = self.attached();
        if attached == self.streams.len() && matches!(self.state, ConnectionState::Connected { .. })
        {
            return;
        }
        self.state = if attached == self.streams.len() {
            ConnectionState::Connected { since: Moment::Observed(now_ns) }
        } else {
            ConnectionState::Degraded {
                since: Moment::Observed(now_ns),
                reason: DegradedReason::SubscriptionsIncomplete {
                    attached,
                    requested: self.streams.len(),
                },
            }
        };
    }

    fn on_tick(&mut self, now_ns: i128, out: &mut Vec<Action>) {
        if matches!(self.state, ConnectionState::Disconnected) {
            self.state = ConnectionState::Connecting;
            self.deadline_ns = Some(now_ns + LINK_SILENT_NS);
            out.push(Action::Connect);
            return;
        }

        if now_ns - self.last_inbound_ns >= LINK_SILENT_NS {
            out.push(Action::Disconnect);
            out.push(Action::ResetBooks);
            self.retreat(now_ns);
            return;
        }

        if now_ns - self.last_ping_ns >= PING_INTERVAL_NS {
            self.last_ping_ns = now_ns;
            out.push(Action::Send(Self::ping_frame()));
        }

        // Ask a quiet stream again. A reconnect would discard every healthy one.
        let mut exhausted = false;
        let mut again = Vec::new();
        for (instrument, stream) in &mut self.streams {
            let quiet_since = stream.seen.unwrap_or(stream.asked_at);
            if now_ns - quiet_since < STREAM_SILENT_NS {
                continue;
            }
            if stream.attempts >= RESUBSCRIBE_LIMIT {
                exhausted = true;
                break;
            }
            stream.attempts += 1;
            stream.seen = None;
            stream.asked_at = now_ns;
            again.push(instrument.clone());
        }

        if exhausted {
            out.push(Action::Disconnect);
            out.push(Action::ResetBooks);
            self.retreat(now_ns);
            return;
        }

        for instrument in &again {
            out.extend(Self::rejoin_frames(instrument).map(Action::Send));
        }
        if !again.is_empty() {
            self.settle(now_ns);
        }

        self.deadline_ns = Some(now_ns + STREAM_SILENT_NS.min(PING_INTERVAL_NS));
    }

    fn retreat(&mut self, now_ns: i128) {
        self.attempts = self.attempts.saturating_add(1);
        self.state = ConnectionState::Disconnected;
        for stream in self.streams.values_mut() {
            stream.seen = None;
            stream.asked_at = now_ns;
            stream.attempts = 0;
        }

        self.deadline_ns = Some(now_ns + RECONNECT.delay_ns(self.attempts, self.jitter.as_mut()));
    }
}
