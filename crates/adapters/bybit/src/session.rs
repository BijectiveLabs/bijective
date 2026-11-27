// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Connection session as a state machine that touches nothing, so every failure path can be driven.
//! Design notes: `docs/integrations/bybit.md`.

use bijective_venue::resilience::{Backoff, Jitter};

use crate::message::{MAX_TOPICS_PER_CONNECTION, PING_INTERVAL_NS, VenueMessage};

/// Without a deadline the confirmed-subscription rule cannot detect a venue that accepts the socket and sends nothing.
pub const SUBSCRIBE_ACK_TIMEOUT_NS: i128 = 10_000_000_000;

/// Two ping intervals: no frame for that long is a half-open link, not a quiet market.
pub const INBOUND_IDLE_LIMIT_NS: i128 = 2 * PING_INTERVAL_NS;

/// Held here so every session deadline lives in one place.
pub const CONNECT_TIMEOUT_NS: i128 = 15_000_000_000;

/// Three seconds doubling to five minutes. Checked when the crate is built, not when it runs.
pub(crate) const RECONNECT: Backoff = match Backoff::new(3_000_000_000, 300_000_000_000) {
    Ok(backoff) => backoff,
    Err(_) => panic!("the reconnect backoff constants are valid"),
};

/// Fails closed for an operator instead of retrying forever, so a dead component says so.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Down,
    Connecting,
    /// Attached at the socket, not yet confirmed. Book traffic here is not authoritative.
    Subscribing,
    Live,
    Degraded(Degraded),
    Failed(Failure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degraded {
    /// Books are dropped and the subscription rebuilt. Reported, never silent.
    SequenceGap,
    ConnectTimedOut,
    SubscribeRefused,
    SubscribeUnanswered,
    LinkSilent,
    SocketClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    ReconnectAttemptsExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<'a> {
    Tick,
    Connected,
    ConnectFailed,
    Inbound(&'a VenueMessage),
    SocketClosed,
    /// Raised by the book component, since the transport cannot see a sequence gap.
    SequenceGap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Connect,
    Send(String),
    Disconnect,
    /// Forget every book. A book carried across a gap is quietly wrong.
    ResetBooks,
    Report(ConnectionState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    NoTopics,
    TooManyTopics { asked: usize, limit: usize },
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoTopics => write!(f, "a session with no topics subscribes to nothing"),
            Self::TooManyTopics { asked, limit } => {
                write!(f, "{asked} topics asked of a connection that carries {limit}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug)]
pub struct Session {
    jitter: Box<dyn Jitter>,
    topics: Vec<String>,
    state: ConnectionState,
    attempts: u32,
    /// Next timed thing due. `None` means waiting on the network.
    deadline_ns: Option<i128>,
    last_inbound_ns: i128,
    last_ping_ns: i128,
}

impl Session {
    /// Without jitter, so a test's deadlines are exact. Production builds the session with
    /// `with_jitter`. There is no other way to make one outside a test.
    #[cfg(test)]
    pub fn new(topics: Vec<String>, now_ns: i128) -> Result<Self, SessionError> {
        Self::with_jitter(topics, now_ns, Box::new(bijective_venue::resilience::Fixed(u64::MAX)))
    }

    pub fn with_jitter(
        topics: Vec<String>,
        now_ns: i128,
        jitter: Box<dyn Jitter>,
    ) -> Result<Self, SessionError> {
        if topics.is_empty() {
            return Err(SessionError::NoTopics);
        }
        if topics.len() > MAX_TOPICS_PER_CONNECTION {
            return Err(SessionError::TooManyTopics {
                asked: topics.len(),
                limit: MAX_TOPICS_PER_CONNECTION,
            });
        }

        Ok(Self {
            jitter,
            topics,
            state: ConnectionState::Down,
            attempts: 0,
            deadline_ns: Some(now_ns),
            last_inbound_ns: now_ns,
            last_ping_ns: now_ns,
        })
    }

    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    /// When the driver should call `step` with `Tick` if nothing else happens first.
    pub fn wake_at(&self) -> Option<i128> {
        self.deadline_ns
    }

    pub fn subscribe_frame(&self) -> String {
        serde_json::json!({ "op": "subscribe", "args": self.topics }).to_string()
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
                self.state = ConnectionState::Subscribing;
                self.deadline_ns = Some(now_ns + SUBSCRIBE_ACK_TIMEOUT_NS);
                out.push(Action::Send(self.subscribe_frame()));
            }
            Event::ConnectFailed => self.retreat(now_ns, Degraded::SocketClosed),
            Event::SocketClosed => self.retreat(now_ns, Degraded::SocketClosed),
            Event::SequenceGap => {
                out.push(Action::ResetBooks);
                self.retreat(now_ns, Degraded::SequenceGap);
            }
            Event::Inbound(msg) => self.on_inbound(msg, now_ns, &mut out),
        }

        // Report FIRST: `Connect` earlier in the list would otherwise surface states out of order.
        if self.state != before {
            out.insert(0, Action::Report(self.state.clone()));
        }
        out
    }

    fn on_tick(&mut self, now_ns: i128, out: &mut Vec<Action>) {
        match self.state {
            ConnectionState::Down | ConnectionState::Degraded(_) => {
                if self.deadline_ns.is_some_and(|d| now_ns >= d) {
                    if self.attempts >= MAX_RECONNECT_ATTEMPTS {
                        self.state = ConnectionState::Failed(Failure::ReconnectAttemptsExhausted);
                        self.deadline_ns = None;
                        return;
                    }
                    self.state = ConnectionState::Connecting;
                    self.deadline_ns = Some(now_ns + CONNECT_TIMEOUT_NS);
                    out.push(Action::Connect);
                }
            }
            ConnectionState::Subscribing => {
                if self.deadline_ns.is_some_and(|d| now_ns >= d) {
                    out.push(Action::Disconnect);
                    self.retreat(now_ns, Degraded::SubscribeUnanswered);
                }
            }
            ConnectionState::Live => {
                if now_ns - self.last_inbound_ns >= INBOUND_IDLE_LIMIT_NS {
                    out.push(Action::Disconnect);
                    out.push(Action::ResetBooks);
                    self.retreat(now_ns, Degraded::LinkSilent);
                    return;
                }
                if now_ns - self.last_ping_ns >= PING_INTERVAL_NS {
                    self.last_ping_ns = now_ns;
                    out.push(Action::Send(r#"{"op":"ping"}"#.to_string()));
                }
                self.deadline_ns = Some(self.next_live_deadline());
            }
            ConnectionState::Connecting => {
                if self.deadline_ns.is_some_and(|d| now_ns >= d) {
                    out.push(Action::Disconnect);
                    self.retreat(now_ns, Degraded::ConnectTimedOut);
                }
            }
            ConnectionState::Failed(_) => {}
        }
    }

    fn on_inbound(&mut self, msg: &VenueMessage, now_ns: i128, out: &mut Vec<Action>) {
        self.last_inbound_ns = now_ns;

        match msg {
            VenueMessage::SubscribeResult { success, .. } => {
                if matches!(self.state, ConnectionState::Subscribing) {
                    if *success {
                        self.state = ConnectionState::Live;
                        self.last_ping_ns = now_ns;
                        self.deadline_ns = Some(self.next_live_deadline());
                    } else {
                        out.push(Action::Disconnect);
                        self.retreat(now_ns, Degraded::SubscribeRefused);
                    }
                }
            }
            _ => {
                if matches!(self.state, ConnectionState::Live) {
                    self.deadline_ns = Some(self.next_live_deadline());
                }
            }
        }
    }

    /// Step back to a waiting state with the reason attached, and schedule the retry.
    fn retreat(&mut self, now_ns: i128, why: Degraded) {
        if matches!(self.state, ConnectionState::Failed(_)) {
            return;
        }
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts >= MAX_RECONNECT_ATTEMPTS {
            self.state = ConnectionState::Failed(Failure::ReconnectAttemptsExhausted);
            self.deadline_ns = None;
            return;
        }
        self.state = ConnectionState::Degraded(why);
        self.deadline_ns = Some(now_ns + self.backoff_ns());
    }

    /// Exponential up to the ceiling, spread so that clients do not retry in step.
    fn backoff_ns(&mut self) -> i128 {
        RECONNECT.delay_ns(self.attempts, self.jitter.as_mut())
    }

    fn next_live_deadline(&self) -> i128 {
        let ping_due = self.last_ping_ns + PING_INTERVAL_NS;
        let idle_due = self.last_inbound_ns + INBOUND_IDLE_LIMIT_NS;
        ping_due.min(idle_due)
    }
}
