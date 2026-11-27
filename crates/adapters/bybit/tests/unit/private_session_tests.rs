// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The private stream's session driven through every path without a socket.

use super::*;
use bijective_venue::resilience::Fixed;

const S: i128 = 1_000_000_000;

fn session() -> PrivateSession {
    PrivateSession::new(0, Box::new(Fixed(u64::MAX)))
}

fn authenticated(success: bool) -> PrivateMessage {
    PrivateMessage::Authenticated { success, detail: String::new() }
}

fn subscribed(success: bool) -> PrivateMessage {
    PrivateMessage::Subscribed { success, detail: String::new() }
}

/// Connects, authenticates and subscribes, returning the actions of the final step.
fn live(s: &mut PrivateSession) -> Vec<PrivateAction> {
    assert!(s.step(PrivateEvent::Tick, 0).contains(&PrivateAction::Connect));
    assert!(s.step(PrivateEvent::Connected, 0).contains(&PrivateAction::Authenticate));
    let auth = authenticated(true);
    let sent = s.step(PrivateEvent::Inbound(&auth), 0);
    assert!(sent.contains(&PrivateAction::Send(subscribe_frame())), "{sent:?}");
    let ack = subscribed(true);
    s.step(PrivateEvent::Inbound(&ack), 0)
}

#[test]
fn attaching_reports_live_and_asks_for_a_read_of_what_the_gap_hid() {
    let mut s = session();
    let actions = live(&mut s);
    assert_eq!(s.state(), PrivateState::Live);
    assert_eq!(actions, [PrivateAction::Report(PrivateState::Live), PrivateAction::Changed]);
}

#[test]
fn the_subscription_waits_for_authentication() {
    let mut s = session();
    s.step(PrivateEvent::Tick, 0);
    let actions = s.step(PrivateEvent::Connected, 0);
    assert!(!actions.iter().any(|a| matches!(a, PrivateAction::Send(_))), "{actions:?}");
    assert_eq!(s.state(), PrivateState::Authenticating);
}

#[test]
fn an_order_or_execution_report_asks_for_a_read() {
    let mut s = session();
    live(&mut s);
    let orders = PrivateMessage::Orders(Vec::new());
    let executions = PrivateMessage::Executions(Vec::new());
    assert_eq!(s.step(PrivateEvent::Inbound(&orders), S), [PrivateAction::Changed]);
    assert_eq!(s.step(PrivateEvent::Inbound(&executions), S), [PrivateAction::Changed]);
    assert_eq!(s.step(PrivateEvent::Inbound(&PrivateMessage::Pong), S), []);
}

/// An unreadable frame may have been a report. A needless read is cheaper than a missed fill.
#[test]
fn an_unreadable_frame_asks_for_a_read() {
    let mut s = session();
    live(&mut s);
    assert_eq!(s.step(PrivateEvent::Unreadable, S), [PrivateAction::Changed]);
}

#[test]
fn a_refused_authentication_disconnects_and_retries_later() {
    let mut s = session();
    s.step(PrivateEvent::Tick, 0);
    s.step(PrivateEvent::Connected, 0);
    let refused = authenticated(false);
    let actions = s.step(PrivateEvent::Inbound(&refused), S);
    assert_eq!(s.state(), PrivateState::Lapsed(Lapse::AuthRefused));
    assert!(actions.contains(&PrivateAction::Disconnect));
    let retry_at = s.wake_at().expect("a retry is scheduled");
    assert!(retry_at > S, "{retry_at}");
    assert!(s.step(PrivateEvent::Tick, retry_at).contains(&PrivateAction::Connect));
}

#[test]
fn an_unanswered_authentication_or_subscription_is_given_up_on() {
    let mut s = session();
    s.step(PrivateEvent::Tick, 0);
    s.step(PrivateEvent::Connected, 0);
    s.step(PrivateEvent::Tick, AUTH_ACK_TIMEOUT_NS);
    assert_eq!(s.state(), PrivateState::Lapsed(Lapse::AuthUnanswered));

    let mut s = session();
    s.step(PrivateEvent::Tick, 0);
    s.step(PrivateEvent::Connected, 0);
    s.step(PrivateEvent::Inbound(&authenticated(true)), 0);
    s.step(PrivateEvent::Tick, SUBSCRIBE_ACK_TIMEOUT_NS);
    assert_eq!(s.state(), PrivateState::Lapsed(Lapse::SubscribeUnanswered));
}

#[test]
fn a_refused_subscription_lapses() {
    let mut s = session();
    s.step(PrivateEvent::Tick, 0);
    s.step(PrivateEvent::Connected, 0);
    s.step(PrivateEvent::Inbound(&authenticated(true)), 0);
    s.step(PrivateEvent::Inbound(&subscribed(false)), 0);
    assert_eq!(s.state(), PrivateState::Lapsed(Lapse::SubscribeRefused));
}

#[test]
fn a_live_link_pings_and_a_silent_one_lapses() {
    let mut s = session();
    live(&mut s);
    let ping = s.step(PrivateEvent::Tick, PING_INTERVAL_NS);
    assert_eq!(ping, [PrivateAction::Send(PING_FRAME.to_string())]);
    let silent = s.step(PrivateEvent::Tick, INBOUND_IDLE_LIMIT_NS);
    assert_eq!(s.state(), PrivateState::Lapsed(Lapse::LinkSilent));
    assert!(silent.contains(&PrivateAction::Disconnect));
}

/// The evidence cadence covers a lapsed link, so the session keeps retrying instead of failing.
#[test]
fn the_link_never_stops_retrying() {
    let mut s = session();
    let mut now = 0;
    for _ in 0..100 {
        s.step(PrivateEvent::Tick, now);
        assert_eq!(s.state(), PrivateState::Connecting);
        s.step(PrivateEvent::ConnectFailed, now);
        now = s.wake_at().expect("a retry is always scheduled");
    }
    assert_eq!(s.state(), PrivateState::Lapsed(Lapse::SocketClosed));
}
