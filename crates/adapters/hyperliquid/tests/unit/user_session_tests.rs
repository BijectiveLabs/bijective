// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The account stream's session driven through every path without a socket.

use super::*;
use bijective_venue::resilience::Fixed;

const USER: &str = "0x0000000000000000000000000000000000000001";

fn session() -> UserSession {
    UserSession::new(USER, 0, Box::new(Fixed(u64::MAX)))
}

fn live(s: &mut UserSession) -> Vec<UserAction> {
    assert!(s.step(UserEvent::Tick, 0).contains(&UserAction::Connect));
    let sent = s.step(UserEvent::Connected, 0);
    let frames: Vec<UserAction> =
        subscribe_frames(USER).into_iter().map(UserAction::Send).collect();
    assert_eq!(sent[1..], frames[..], "{sent:?}");
    assert_eq!(s.step(UserEvent::Inbound(UserMessage::Acknowledged), 0), []);
    s.step(UserEvent::Inbound(UserMessage::Acknowledged), 0)
}

#[test]
fn the_subscriptions_name_the_account() {
    let frames = subscribe_frames(USER);
    assert_eq!(
        frames[0],
        r#"{"method":"subscribe","subscription":{"type":"orderUpdates","user":"0x0000000000000000000000000000000000000001"}}"#
    );
    assert!(frames[1].contains(r#""type":"userFills""#));
}

#[test]
fn the_link_is_live_only_when_every_subscription_is_acknowledged() {
    let mut s = session();
    let actions = live(&mut s);
    assert_eq!(s.state(), UserState::Live);
    assert_eq!(actions, [UserAction::Report(UserState::Live), UserAction::Changed]);
}

#[test]
fn frames_are_read_by_channel() {
    assert_eq!(classify(r#"{"channel":"orderUpdates","data":[]}"#), Some(UserMessage::Reported));
    assert_eq!(
        classify(r#"{"channel":"userFills","data":{"isSnapshot":true,"user":"0x1","fills":[]}}"#),
        Some(UserMessage::Reported)
    );
    assert_eq!(classify(r#"{"channel":"pong"}"#), Some(UserMessage::Pong));
    assert_eq!(classify(r#"{"channel":"error","data":"x"}"#), Some(UserMessage::Refused));
    assert_eq!(classify(r#"{"channel":"l2Book","data":{}}"#), Some(UserMessage::Other));
    assert_eq!(classify("not json"), None);
}

#[test]
fn a_report_or_an_unreadable_frame_asks_for_a_read() {
    let mut s = session();
    live(&mut s);
    assert_eq!(s.step(UserEvent::Inbound(UserMessage::Reported), 1), [UserAction::Changed]);
    assert_eq!(s.step(UserEvent::Unreadable, 1), [UserAction::Changed]);
    assert_eq!(s.step(UserEvent::Inbound(UserMessage::Pong), 1), []);
}

#[test]
fn a_refused_or_unanswered_subscription_lapses() {
    let mut s = session();
    s.step(UserEvent::Tick, 0);
    s.step(UserEvent::Connected, 0);
    let refused = s.step(UserEvent::Inbound(UserMessage::Refused), 0);
    assert!(refused.contains(&UserAction::Disconnect));
    assert_eq!(s.state(), UserState::Lapsed(Lapse::SubscribeRefused));

    let mut s = session();
    s.step(UserEvent::Tick, 0);
    s.step(UserEvent::Connected, 0);
    s.step(UserEvent::Inbound(UserMessage::Acknowledged), 0);
    s.step(UserEvent::Tick, ANSWER_TIMEOUT_NS);
    assert_eq!(s.state(), UserState::Lapsed(Lapse::SubscribeUnanswered));
}

#[test]
fn a_live_link_pings_and_a_silent_one_lapses() {
    let mut s = session();
    live(&mut s);
    let ping = s.step(UserEvent::Tick, PING_INTERVAL_NS);
    assert_eq!(ping, [UserAction::Send(r#"{"method":"ping"}"#.to_string())]);
    s.step(UserEvent::Tick, LINK_SILENT_NS);
    assert_eq!(s.state(), UserState::Lapsed(Lapse::LinkSilent));
}

#[test]
fn the_link_never_stops_retrying() {
    let mut s = session();
    let mut now = 0;
    for _ in 0..100 {
        s.step(UserEvent::Tick, now);
        assert_eq!(s.state(), UserState::Connecting);
        s.step(UserEvent::ConnectFailed, now);
        now = s.wake_at().expect("a retry is always scheduled");
    }
}
