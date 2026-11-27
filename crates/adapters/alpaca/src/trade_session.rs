// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The account's trade updates stream as a state machine that touches nothing (WebSocket
//! streaming, read 2026-11-17, `docs/integrations/alpaca.md`).
//!
//! The stream carries no order state upward. It only prompts a read of the venue's evidence, so a
//! lapsed stream costs latency and never correctness: it retries at the backoff ceiling for as long
//! as the node runs. The venue states no heartbeat, so a WebSocket ping keeps the link.

use bijective_venue::resilience::{Backoff, Jitter};
use serde_json::Value;

/// The answers to authentication and to the listen request are each given this long.
pub const ANSWER_TIMEOUT_NS: i128 = 10_000_000_000;

/// A WebSocket ping every 30 seconds. Its pong counts as traffic.
pub const PING_INTERVAL_NS: i128 = 30_000_000_000;

/// Three pings unanswered and no message: the link is gone.
pub const LINK_SILENT_NS: i128 = 90_000_000_000;

const RECONNECT: Backoff = match Backoff::new(1_000_000_000, 300_000_000_000) {
    Ok(backoff) => backoff,
    Err(_) => panic!("the reconnect backoff constants are valid"),
};

/// What one frame is, read from its `stream` (and `action` for an error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeMessage {
    Authorized,
    Unauthorized,
    /// The listen request was acknowledged with `trade_updates` among the streams.
    Listening,
    /// An order event: new, fill, partial fill, canceled and the rest.
    Reported,
    /// The venue's error, sent before it closes the connection.
    Refused,
    Other,
}

/// `None` when the frame is not a JSON object: it may have been a report.
#[must_use]
pub fn classify(frame: &[u8]) -> Option<TradeMessage> {
    let value: Value = serde_json::from_slice(frame).ok()?;
    if value.get("action").and_then(Value::as_str) == Some("error") {
        return Some(TradeMessage::Refused);
    }
    Some(match value.get("stream")?.as_str()? {
        "authorization" => match value.pointer("/data/status").and_then(Value::as_str) {
            Some("authorized") => TradeMessage::Authorized,
            _ => TradeMessage::Unauthorized,
        },
        "listening" => {
            let streams = value.pointer("/data/streams").and_then(Value::as_array);
            let listed = streams.is_some_and(|s| s.iter().any(|n| n == "trade_updates"));
            if listed { TradeMessage::Listening } else { TradeMessage::Refused }
        }
        "trade_updates" => TradeMessage::Reported,
        _ => TradeMessage::Other,
    })
}

#[must_use]
pub fn listen_frame() -> String {
    serde_json::json!({ "action": "listen", "data": { "streams": ["trade_updates"] } }).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeState {
    Down,
    Connecting,
    Authenticating,
    Listening,
    Live,
    Lapsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeEvent {
    Tick,
    Connected,
    ConnectFailed,
    SocketClosed,
    Pong,
    Inbound(TradeMessage),
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeAction {
    Connect,
    /// Sends the authentication message, which the driver holds because it carries the secret.
    Authenticate,
    Send(String),
    Ping,
    Disconnect,
    /// The venue reported a change, or the link attached after a gap that could hide one.
    Changed,
    Report(TradeState),
}

#[derive(Debug)]
pub struct TradeSession {
    jitter: Box<dyn Jitter>,
    state: TradeState,
    attempts: u32,
    deadline_ns: Option<i128>,
    last_inbound_ns: i128,
    last_ping_ns: i128,
}

impl TradeSession {
    pub fn new(now_ns: i128, jitter: Box<dyn Jitter>) -> Self {
        Self {
            jitter,
            state: TradeState::Down,
            attempts: 0,
            deadline_ns: Some(now_ns),
            last_inbound_ns: now_ns,
            last_ping_ns: now_ns,
        }
    }

    #[must_use]
    pub fn state(&self) -> TradeState {
        self.state
    }

    #[must_use]
    pub fn wake_at(&self) -> Option<i128> {
        self.deadline_ns
    }

    pub fn step(&mut self, event: TradeEvent, now_ns: i128) -> Vec<TradeAction> {
        let mut out = Vec::new();
        let before = self.state;
        match event {
            TradeEvent::Tick => self.on_tick(now_ns, &mut out),
            TradeEvent::Connected => {
                self.last_inbound_ns = now_ns;
                self.state = TradeState::Authenticating;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(TradeAction::Authenticate);
            }
            TradeEvent::ConnectFailed | TradeEvent::SocketClosed => self.retreat(now_ns),
            TradeEvent::Pong => self.last_inbound_ns = now_ns,
            TradeEvent::Unreadable => {
                self.last_inbound_ns = now_ns;
                out.push(TradeAction::Changed);
            }
            TradeEvent::Inbound(message) => self.on_inbound(message, now_ns, &mut out),
        }
        if self.state != before {
            out.insert(0, TradeAction::Report(self.state));
        }
        out
    }

    fn on_inbound(&mut self, message: TradeMessage, now_ns: i128, out: &mut Vec<TradeAction>) {
        self.last_inbound_ns = now_ns;
        match (message, self.state) {
            (TradeMessage::Authorized, TradeState::Authenticating) => {
                self.state = TradeState::Listening;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(TradeAction::Send(listen_frame()));
            }
            (TradeMessage::Listening, TradeState::Listening) => {
                self.state = TradeState::Live;
                self.attempts = 0;
                self.last_ping_ns = now_ns;
                self.deadline_ns = Some(self.next_live_deadline());
                // Whatever happened while the link was down was not pushed.
                out.push(TradeAction::Changed);
            }
            (TradeMessage::Unauthorized | TradeMessage::Refused, _) => {
                out.push(TradeAction::Disconnect);
                self.retreat(now_ns);
            }
            (TradeMessage::Reported, _) => out.push(TradeAction::Changed),
            _ => {}
        }
    }

    fn on_tick(&mut self, now_ns: i128, out: &mut Vec<TradeAction>) {
        let due = self.deadline_ns.is_some_and(|d| now_ns >= d);
        match self.state {
            TradeState::Down | TradeState::Lapsed if due => {
                self.state = TradeState::Connecting;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(TradeAction::Connect);
            }
            TradeState::Connecting | TradeState::Authenticating | TradeState::Listening if due => {
                out.push(TradeAction::Disconnect);
                self.retreat(now_ns);
            }
            TradeState::Live => {
                if now_ns - self.last_inbound_ns >= LINK_SILENT_NS {
                    out.push(TradeAction::Disconnect);
                    self.retreat(now_ns);
                    return;
                }
                if now_ns - self.last_ping_ns >= PING_INTERVAL_NS {
                    self.last_ping_ns = now_ns;
                    out.push(TradeAction::Ping);
                }
                self.deadline_ns = Some(self.next_live_deadline());
            }
            _ => {}
        }
    }

    fn retreat(&mut self, now_ns: i128) {
        self.attempts = self.attempts.saturating_add(1);
        self.state = TradeState::Lapsed;
        self.deadline_ns = Some(now_ns + RECONNECT.delay_ns(self.attempts, self.jitter.as_mut()));
    }

    fn next_live_deadline(&self) -> i128 {
        (self.last_ping_ns + PING_INTERVAL_NS).min(self.last_inbound_ns + LINK_SILENT_NS)
    }
}

#[cfg(test)]
#[path = "../tests/unit/trade_session_tests.rs"]
mod tests;
