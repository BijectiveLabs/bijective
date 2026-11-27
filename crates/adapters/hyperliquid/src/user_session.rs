// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The account's order and fill stream (`orderUpdates`, `userFills`) as a state machine that
//! touches nothing. Sources: WebSocket and Subscriptions pages, read 2026-11-16, in
//! `docs/integrations/hyperliquid.md`.
//!
//! The stream carries no order state upward. It only prompts a read of the venue's evidence, so a
//! lapsed stream costs latency and never correctness: it retries at the backoff ceiling for as long
//! as the node runs instead of failing for an operator. The subscriptions need no signature: they
//! name the account's address, which is public.

use bijective_venue::resilience::{Backoff, Jitter};

use crate::message::PING_INTERVAL_NS;
use crate::session::LINK_SILENT_NS;

/// The subscriptions the stream holds. Each is acknowledged by one `subscriptionResponse`.
pub const KINDS: [&str; 2] = ["orderUpdates", "userFills"];

/// The handshake and the acknowledgements are each given this long.
pub const ANSWER_TIMEOUT_NS: i128 = 10_000_000_000;

/// One second doubling to five minutes: the evidence cadence covers the gap, so there is no hurry.
const RECONNECT: Backoff = match Backoff::new(1_000_000_000, 300_000_000_000) {
    Ok(backoff) => backoff,
    Err(_) => panic!("the reconnect backoff constants are valid"),
};

/// What a frame on this stream is, read from its channel alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserMessage {
    Acknowledged,
    Pong,
    /// An order update or fills, the opening snapshot included.
    Reported,
    /// The venue's free-text error. It names no cause.
    Refused,
    Other,
}

/// `None` when the frame is not a JSON object with a channel: it may have been a report.
#[must_use]
pub fn classify(frame: &str) -> Option<UserMessage> {
    let value: serde_json::Value = serde_json::from_str(frame).ok()?;
    Some(match value.get("channel")?.as_str()? {
        "subscriptionResponse" => UserMessage::Acknowledged,
        "pong" => UserMessage::Pong,
        "orderUpdates" | "userFills" => UserMessage::Reported,
        "error" => UserMessage::Refused,
        _ => UserMessage::Other,
    })
}

#[must_use]
pub fn subscribe_frames(user: &str) -> Vec<String> {
    KINDS
        .iter()
        .map(|kind| {
            serde_json::json!({ "method": "subscribe", "subscription": { "type": kind, "user": user } })
                .to_string()
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserState {
    Down,
    Connecting,
    Subscribing,
    Live,
    Lapsed(Lapse),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lapse {
    SocketClosed,
    ConnectTimedOut,
    SubscribeRefused,
    SubscribeUnanswered,
    LinkSilent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEvent {
    Tick,
    Connected,
    ConnectFailed,
    SocketClosed,
    Inbound(UserMessage),
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserAction {
    Connect,
    Send(String),
    Disconnect,
    /// The venue reported a change, or the link attached after a gap that could hide one.
    Changed,
    Report(UserState),
}

#[derive(Debug)]
pub struct UserSession {
    jitter: Box<dyn Jitter>,
    user: String,
    state: UserState,
    attempts: u32,
    acknowledged: usize,
    deadline_ns: Option<i128>,
    last_inbound_ns: i128,
    last_ping_ns: i128,
}

impl UserSession {
    pub fn new(user: impl Into<String>, now_ns: i128, jitter: Box<dyn Jitter>) -> Self {
        Self {
            jitter,
            user: user.into(),
            state: UserState::Down,
            attempts: 0,
            acknowledged: 0,
            deadline_ns: Some(now_ns),
            last_inbound_ns: now_ns,
            last_ping_ns: now_ns,
        }
    }

    #[must_use]
    pub fn state(&self) -> UserState {
        self.state
    }

    #[must_use]
    pub fn wake_at(&self) -> Option<i128> {
        self.deadline_ns
    }

    pub fn step(&mut self, event: UserEvent, now_ns: i128) -> Vec<UserAction> {
        let mut out = Vec::new();
        let before = self.state;
        match event {
            UserEvent::Tick => self.on_tick(now_ns, &mut out),
            UserEvent::Connected => {
                self.attempts = 0;
                self.acknowledged = 0;
                self.last_inbound_ns = now_ns;
                self.last_ping_ns = now_ns;
                self.state = UserState::Subscribing;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.extend(subscribe_frames(&self.user).into_iter().map(UserAction::Send));
            }
            UserEvent::ConnectFailed | UserEvent::SocketClosed => {
                self.retreat(now_ns, Lapse::SocketClosed);
            }
            UserEvent::Inbound(message) => self.on_inbound(message, now_ns, &mut out),
            UserEvent::Unreadable => {
                self.last_inbound_ns = now_ns;
                out.push(UserAction::Changed);
            }
        }
        if self.state != before {
            out.insert(0, UserAction::Report(self.state));
        }
        out
    }

    fn on_tick(&mut self, now_ns: i128, out: &mut Vec<UserAction>) {
        let due = self.deadline_ns.is_some_and(|d| now_ns >= d);
        match self.state {
            UserState::Down | UserState::Lapsed(_) if due => {
                self.state = UserState::Connecting;
                self.deadline_ns = Some(now_ns + ANSWER_TIMEOUT_NS);
                out.push(UserAction::Connect);
            }
            UserState::Connecting if due => self.give_up(now_ns, Lapse::ConnectTimedOut, out),
            UserState::Subscribing if due => {
                self.give_up(now_ns, Lapse::SubscribeUnanswered, out);
            }
            UserState::Live => {
                if now_ns - self.last_inbound_ns >= LINK_SILENT_NS {
                    self.give_up(now_ns, Lapse::LinkSilent, out);
                    return;
                }
                if now_ns - self.last_ping_ns >= PING_INTERVAL_NS {
                    self.last_ping_ns = now_ns;
                    out.push(UserAction::Send(serde_json::json!({ "method": "ping" }).to_string()));
                }
                self.deadline_ns = Some(self.next_live_deadline());
            }
            _ => {}
        }
    }

    fn on_inbound(&mut self, message: UserMessage, now_ns: i128, out: &mut Vec<UserAction>) {
        self.last_inbound_ns = now_ns;
        match (message, self.state) {
            (UserMessage::Acknowledged, UserState::Subscribing) => {
                self.acknowledged += 1;
                if self.acknowledged == KINDS.len() {
                    self.state = UserState::Live;
                    self.last_ping_ns = now_ns;
                    // Whatever happened while the link was down was not pushed.
                    out.push(UserAction::Changed);
                }
            }
            (UserMessage::Refused, UserState::Subscribing) => {
                self.give_up(now_ns, Lapse::SubscribeRefused, out);
            }
            (UserMessage::Reported, _) => out.push(UserAction::Changed),
            _ => {}
        }
        if self.state == UserState::Live {
            self.deadline_ns = Some(self.next_live_deadline());
        }
    }

    fn give_up(&mut self, now_ns: i128, why: Lapse, out: &mut Vec<UserAction>) {
        out.push(UserAction::Disconnect);
        self.retreat(now_ns, why);
    }

    /// No final failure: the evidence cadence covers a lapsed link, and the backoff bounds the
    /// reconnects.
    fn retreat(&mut self, now_ns: i128, why: Lapse) {
        self.attempts = self.attempts.saturating_add(1);
        self.state = UserState::Lapsed(why);
        self.deadline_ns = Some(now_ns + RECONNECT.delay_ns(self.attempts, self.jitter.as_mut()));
    }

    fn next_live_deadline(&self) -> i128 {
        (self.last_ping_ns + PING_INTERVAL_NS).min(self.last_inbound_ns + LINK_SILENT_NS)
    }
}

#[cfg(test)]
#[path = "../tests/unit/user_session_tests.rs"]
mod tests;
