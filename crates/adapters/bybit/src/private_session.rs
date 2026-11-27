// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The private stream's connection as a state machine that touches nothing: connect, authenticate,
//! subscribe, keep the link alive, and say when the venue reported a change. Design notes:
//! `docs/integrations/bybit.md`.
//!
//! The session carries no order state upward: it prompts a read of the venue's evidence, and the
//! link keeps pushed fills for that read. A lapsed stream costs latency and never correctness: it
//! retries at the backoff ceiling for as long as the node runs instead of failing for an operator.

use bijective_venue::resilience::Jitter;

use crate::message::PING_INTERVAL_NS;
use crate::private::{PrivateMessage, subscribe_frame};
use crate::session::{
    CONNECT_TIMEOUT_NS, INBOUND_IDLE_LIMIT_NS, RECONNECT, SUBSCRIBE_ACK_TIMEOUT_NS,
};

/// The venue's heartbeat (WebSocket connect guide, Heartbeat): a ping every 20 seconds.
pub const PING_FRAME: &str = r#"{"op":"ping"}"#;

/// An authentication the venue does not answer within this is given up on, like a subscription.
pub const AUTH_ACK_TIMEOUT_NS: i128 = SUBSCRIBE_ACK_TIMEOUT_NS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateState {
    Down,
    Connecting,
    Authenticating,
    Subscribing,
    /// Authenticated and subscribed: an order or execution change arrives as it happens.
    Live,
    /// Waiting to reconnect, with the reason the link was lost.
    Lapsed(Lapse),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lapse {
    SocketClosed,
    ConnectTimedOut,
    AuthRefused,
    AuthUnanswered,
    SubscribeRefused,
    SubscribeUnanswered,
    LinkSilent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateEvent<'a> {
    Tick,
    Connected,
    ConnectFailed,
    SocketClosed,
    Inbound(&'a PrivateMessage),
    /// A frame that could not be read. It may have been an order or execution report.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateAction {
    Connect,
    /// Send the authentication frame. The driver signs it, so the secret never enters the session.
    Authenticate,
    Send(String),
    Disconnect,
    /// The venue reported a change, or the link attached after a gap that could hide one.
    Changed,
    Report(PrivateState),
}

#[derive(Debug)]
pub struct PrivateSession {
    jitter: Box<dyn Jitter>,
    state: PrivateState,
    attempts: u32,
    /// Next timed thing due. `None` means waiting on the network.
    deadline_ns: Option<i128>,
    last_inbound_ns: i128,
    last_ping_ns: i128,
}

impl PrivateSession {
    pub fn new(now_ns: i128, jitter: Box<dyn Jitter>) -> Self {
        Self {
            jitter,
            state: PrivateState::Down,
            attempts: 0,
            deadline_ns: Some(now_ns),
            last_inbound_ns: now_ns,
            last_ping_ns: now_ns,
        }
    }

    pub fn state(&self) -> PrivateState {
        self.state
    }

    /// When the driver should call `step` with `Tick` if nothing else happens first.
    pub fn wake_at(&self) -> Option<i128> {
        self.deadline_ns
    }

    pub fn step(&mut self, event: PrivateEvent<'_>, now_ns: i128) -> Vec<PrivateAction> {
        let mut out = Vec::new();
        let before = self.state;
        match event {
            PrivateEvent::Tick => self.on_tick(now_ns, &mut out),
            PrivateEvent::Connected => {
                self.attempts = 0;
                self.last_inbound_ns = now_ns;
                self.last_ping_ns = now_ns;
                self.state = PrivateState::Authenticating;
                self.deadline_ns = Some(now_ns + AUTH_ACK_TIMEOUT_NS);
                out.push(PrivateAction::Authenticate);
            }
            PrivateEvent::ConnectFailed | PrivateEvent::SocketClosed => {
                self.retreat(now_ns, Lapse::SocketClosed);
            }
            PrivateEvent::Inbound(message) => self.on_inbound(message, now_ns, &mut out),
            PrivateEvent::Unreadable => {
                self.last_inbound_ns = now_ns;
                out.push(PrivateAction::Changed);
            }
        }
        if self.state != before {
            out.insert(0, PrivateAction::Report(self.state));
        }
        out
    }

    fn on_tick(&mut self, now_ns: i128, out: &mut Vec<PrivateAction>) {
        let due = self.deadline_ns.is_some_and(|d| now_ns >= d);
        match self.state {
            PrivateState::Down | PrivateState::Lapsed(_) if due => {
                self.state = PrivateState::Connecting;
                self.deadline_ns = Some(now_ns + CONNECT_TIMEOUT_NS);
                out.push(PrivateAction::Connect);
            }
            PrivateState::Connecting if due => self.give_up(now_ns, Lapse::ConnectTimedOut, out),
            PrivateState::Authenticating if due => {
                self.give_up(now_ns, Lapse::AuthUnanswered, out);
            }
            PrivateState::Subscribing if due => {
                self.give_up(now_ns, Lapse::SubscribeUnanswered, out);
            }
            PrivateState::Live => {
                if now_ns - self.last_inbound_ns >= INBOUND_IDLE_LIMIT_NS {
                    self.give_up(now_ns, Lapse::LinkSilent, out);
                    return;
                }
                if now_ns - self.last_ping_ns >= PING_INTERVAL_NS {
                    self.last_ping_ns = now_ns;
                    out.push(PrivateAction::Send(PING_FRAME.to_string()));
                }
                self.deadline_ns = Some(self.next_live_deadline());
            }
            _ => {}
        }
    }

    fn on_inbound(&mut self, message: &PrivateMessage, now_ns: i128, out: &mut Vec<PrivateAction>) {
        self.last_inbound_ns = now_ns;
        match (message, self.state) {
            (PrivateMessage::Authenticated { success: true, .. }, PrivateState::Authenticating) => {
                self.state = PrivateState::Subscribing;
                self.deadline_ns = Some(now_ns + SUBSCRIBE_ACK_TIMEOUT_NS);
                out.push(PrivateAction::Send(subscribe_frame()));
            }
            (
                PrivateMessage::Authenticated { success: false, .. },
                PrivateState::Authenticating,
            ) => {
                self.give_up(now_ns, Lapse::AuthRefused, out);
            }
            (PrivateMessage::Subscribed { success: true, .. }, PrivateState::Subscribing) => {
                self.state = PrivateState::Live;
                self.last_ping_ns = now_ns;
                self.deadline_ns = Some(self.next_live_deadline());
                // Whatever happened while the link was down was not pushed.
                out.push(PrivateAction::Changed);
            }
            (PrivateMessage::Subscribed { success: false, .. }, PrivateState::Subscribing) => {
                self.give_up(now_ns, Lapse::SubscribeRefused, out);
            }
            (PrivateMessage::Orders(_) | PrivateMessage::Executions(_), _) => {
                out.push(PrivateAction::Changed);
            }
            _ => {}
        }
        if self.state == PrivateState::Live {
            self.deadline_ns = Some(self.next_live_deadline());
        }
    }

    fn give_up(&mut self, now_ns: i128, why: Lapse, out: &mut Vec<PrivateAction>) {
        out.push(PrivateAction::Disconnect);
        self.retreat(now_ns, why);
    }

    /// Step back to waiting with the reason attached, and schedule the retry. There is no final
    /// failure: the evidence cadence covers a lapsed link, and the backoff bounds the reconnects.
    fn retreat(&mut self, now_ns: i128, why: Lapse) {
        self.attempts = self.attempts.saturating_add(1);
        self.state = PrivateState::Lapsed(why);
        self.deadline_ns = Some(now_ns + RECONNECT.delay_ns(self.attempts, self.jitter.as_mut()));
    }

    fn next_live_deadline(&self) -> i128 {
        let ping_due = self.last_ping_ns + PING_INTERVAL_NS;
        let idle_due = self.last_inbound_ns + INBOUND_IDLE_LIMIT_NS;
        ping_due.min(idle_due)
    }
}

#[cfg(test)]
#[path = "../tests/unit/private_session_tests.rs"]
mod tests;
