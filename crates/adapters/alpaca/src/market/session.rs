// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The stock stream's link as a state machine that touches nothing: connect, authenticate within
//! the venue's ten seconds, subscribe, and keep the link alive with WebSocket pings, since the
//! venue states no heartbeat and a quiet market sends nothing for hours.

use bijective_venue::contract::{ConnectionState, Moment};
use bijective_venue::resilience::{Backoff, Jitter};

use super::message::{MarketMessage, subscribe_frame};

/// The venue's bound on authentication, and the bound this side keeps on every answer.
pub const ANSWER_TIMEOUT_NS: i128 = 10_000_000_000;

/// A WebSocket ping every 30 seconds. Its pong counts as traffic.
pub const PING_INTERVAL_NS: i128 = 30_000_000_000;

/// Three pings unanswered and no message: the link is gone.
pub const LINK_SILENT_NS: i128 = 90_000_000_000;

/// One second doubling to five minutes. A refused key or the venue's one-connection limit is not
/// cured by retrying faster.
const RECONNECT: Backoff = match Backoff::new(1_000_000_000, 300_000_000_000) {
    Ok(backoff) => backoff,
    Err(_) => panic!("the reconnect backoff constants are valid"),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Connecting,
    Authenticating,
    Subscribing,
    Live,
    Lapsed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent<'a> {
    Tick,
    Connected,
    ConnectFailed,
    SocketClosed,
    /// A control frame answered a ping.
    Pong,
    Inbound(&'a [MarketMessage]),
    /// A frame that could not be read is still traffic.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    Connect,
    Send(String),
    Ping,
    Disconnect,
    Report(ConnectionState),
}

/// No `Debug`: it holds the authentication message, which carries the secret key.
pub struct LinkSession {
    jitter: Box<dyn Jitter>,
    auth: String,
    symbols: Vec<String>,
    state: LinkState,
    attempts: u32,
    deadline_ns: Option<i128>,
    last_inbound_ns: i128,
    last_ping_ns: i128,
    /// The venue's last refusal, kept for the operator's log.
    pub refusal: Option<String>,
}

impl LinkSession {
    pub fn new(auth: String, symbols: Vec<String>, now_ns: i128, jitter: Box<dyn Jitter>) -> Self {
        Self {
            jitter,
            auth,
            symbols,
            state: LinkState::Down,
            attempts: 0,
            deadline_ns: Some(now_ns),
            last_inbound_ns: now_ns,
            last_ping_ns: now_ns,
            refusal: None,
        }
    }

    #[must_use]
    pub fn state(&self) -> LinkState {
        self.state
    }

    #[must_use]
    pub fn wake_at(&self) -> Option<i128> {
        self.deadline_ns
    }

    pub fn step(&mut self, event: LinkEvent<'_>, now_ns: i128) -> Vec<LinkAction> {
        let mut out = Vec::new();
        match event {
            LinkEvent::Tick => self.on_tick(now_ns, &mut out),
            LinkEvent::Connected => {
                self.last_inbound_ns = now_ns;
                self.state = LinkState::Authenticating;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(LinkAction::Send(self.auth.clone()));
            }
            LinkEvent::ConnectFailed | LinkEvent::SocketClosed => self.retreat(now_ns, &mut out),
            LinkEvent::Pong | LinkEvent::Unreadable => self.last_inbound_ns = now_ns,
            LinkEvent::Inbound(messages) => {
                self.last_inbound_ns = now_ns;
                for message in messages {
                    self.on_message(message, now_ns, &mut out);
                }
            }
        }
        out
    }

    fn on_message(&mut self, message: &MarketMessage, now_ns: i128, out: &mut Vec<LinkAction>) {
        // The venue's refusal (a wrong key, its connection limit) ends the link in any state.
        if let MarketMessage::Error { code, message } = message {
            self.refusal = Some(format!("{code} {message}"));
            out.push(LinkAction::Disconnect);
            self.retreat(now_ns, out);
            return;
        }
        match (message, self.state) {
            (MarketMessage::Authenticated, LinkState::Authenticating) => {
                self.state = LinkState::Subscribing;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(LinkAction::Send(subscribe_frame(&self.symbols)));
            }
            (MarketMessage::Subscribed, LinkState::Subscribing) => {
                self.state = LinkState::Live;
                self.attempts = 0;
                self.last_ping_ns = now_ns;
                self.deadline_ns = Some(self.next_live_deadline());
                out.push(LinkAction::Report(ConnectionState::Connected {
                    since: Moment::Observed(now_ns),
                }));
            }
            _ => {}
        }
    }

    fn on_tick(&mut self, now_ns: i128, out: &mut Vec<LinkAction>) {
        let due = self.deadline_ns.is_some_and(|d| now_ns >= d);
        match self.state {
            LinkState::Down | LinkState::Lapsed if due => {
                self.state = LinkState::Connecting;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(LinkAction::Report(ConnectionState::Connecting));
                out.push(LinkAction::Connect);
            }
            LinkState::Connecting | LinkState::Authenticating | LinkState::Subscribing if due => {
                out.push(LinkAction::Disconnect);
                self.retreat(now_ns, out);
            }
            LinkState::Live => {
                if now_ns - self.last_inbound_ns >= LINK_SILENT_NS {
                    out.push(LinkAction::Disconnect);
                    self.retreat(now_ns, out);
                    return;
                }
                if now_ns - self.last_ping_ns >= PING_INTERVAL_NS {
                    self.last_ping_ns = now_ns;
                    out.push(LinkAction::Ping);
                }
                self.deadline_ns = Some(self.next_live_deadline());
            }
            _ => {}
        }
    }

    /// No final failure: the backoff bounds the reconnects, and the state report tells the
    /// consumer the stream is down.
    fn retreat(&mut self, now_ns: i128, out: &mut Vec<LinkAction>) {
        if self.state == LinkState::Lapsed {
            return;
        }
        self.attempts = self.attempts.saturating_add(1);
        self.state = LinkState::Lapsed;
        self.deadline_ns = Some(now_ns + RECONNECT.delay_ns(self.attempts, self.jitter.as_mut()));
        out.push(LinkAction::Report(ConnectionState::Disconnected));
    }

    fn next_live_deadline(&self) -> i128 {
        (self.last_ping_ns + PING_INTERVAL_NS).min(self.last_inbound_ns + LINK_SILENT_NS)
    }
}
