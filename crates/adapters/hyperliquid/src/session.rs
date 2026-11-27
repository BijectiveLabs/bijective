// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Connection session as a state machine that touches nothing, so failure paths run in tests.
//! Differs from the first venue's: unnamed acknowledgement, no numbering, no subscription
//! across a reconnect.

use std::collections::BTreeMap;

use bijective_venue::contract::{ConnectionState, DegradedReason, Moment};
use bijective_venue::resilience::{Backoff, Jitter};

use crate::message::{PING_INTERVAL_NS, VenueMessage};

/// The venue closes a link idle for sixty seconds. The same idle time from it means the link is dead.
pub const LINK_SILENT_NS: i128 = 60_000_000_000;

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
    Inbound(&'a VenueMessage),
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

    /// One coin per subscription on this venue.
    #[must_use]
    pub fn subscribe_frame(instrument: &str) -> String {
        serde_json::json!({
            "method": "subscribe",
            "subscription": { "type": "l2Book", "coin": instrument }
        })
        .to_string()
    }

    /// The trade and context subscriptions beside the book. Not tracked for silence: a quiet market
    /// has no trades, and the context's absence is not a broken link. A reconnect sends them again.
    #[must_use]
    pub fn companion_frames(instrument: &str) -> [String; 2] {
        ["trades", "activeAssetCtx"].map(|kind| {
            serde_json::json!({
                "method": "subscribe",
                "subscription": { "type": kind, "coin": instrument }
            })
            .to_string()
        })
    }

    #[must_use]
    pub fn ping_frame() -> String {
        serde_json::json!({ "method": "ping" }).to_string()
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
                for instrument in self.streams.keys() {
                    out.push(Action::Send(Self::subscribe_frame(instrument)));
                }
                for instrument in self.streams.keys() {
                    out.extend(Self::companion_frames(instrument).map(Action::Send));
                }
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
        }

        // Reported first so a consumer never sees a state following one it was not told about.
        if self.state != before {
            out.insert(0, Action::Report(self.state.clone()));
        }
        out
    }

    fn on_inbound(&mut self, msg: &VenueMessage, now_ns: i128) {
        self.last_inbound_ns = now_ns;

        match msg {
            VenueMessage::Book { symbol, .. } => {
                if let Some(stream) = self.streams.get_mut(symbol) {
                    stream.seen = Some(now_ns);
                    stream.attempts = 0;
                }
                self.settle(now_ns);
            }
            // Confirms nothing: the response names no subscription.
            VenueMessage::SubscriptionAccepted
            | VenueMessage::Trades { .. }
            | VenueMessage::AssetContext { .. }
            | VenueMessage::Pong
            | VenueMessage::Refused { .. }
            | VenueMessage::Ignored => {}
        }
    }

    fn settle(&mut self, now_ns: i128) {
        let attached = self.attached();
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
            out.push(Action::Send(Self::subscribe_frame(instrument)));
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
