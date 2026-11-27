// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The trade updates link on injected events, and the venue's documented frames classified.

use bijective_venue::resilience::SplitMix64;

use super::*;

fn session() -> TradeSession {
    TradeSession::new(0, Box::new(SplitMix64::seeded(1)))
}

#[test]
fn the_documented_frames_are_classified() {
    let read = |frame: &str| classify(frame.as_bytes());
    assert_eq!(
        read(
            r#"{"stream":"authorization","data":{"status":"authorized","action":"authenticate"}}"#
        ),
        Some(TradeMessage::Authorized)
    );
    assert_eq!(
        read(
            r#"{"stream":"authorization","data":{"status":"unauthorized","action":"authenticate"}}"#
        ),
        Some(TradeMessage::Unauthorized)
    );
    assert_eq!(
        read(r#"{"stream":"listening","data":{"streams":["trade_updates"]}}"#),
        Some(TradeMessage::Listening)
    );
    assert_eq!(
        read(r#"{"stream":"listening","data":{"streams":[]}}"#),
        Some(TradeMessage::Refused)
    );
    assert_eq!(
        read(r#"{"stream":"trade_updates","data":{"event":"fill","order":{}}}"#),
        Some(TradeMessage::Reported)
    );
    assert_eq!(
        read(r#"{"action":"error","data":{"error_message":"internal server error"}}"#),
        Some(TradeMessage::Refused)
    );
    assert_eq!(read("not json"), None);
}

/// Authenticate, listen, attach: the attach asks for a read, because the gap could hide a change.
#[test]
fn the_link_authenticates_listens_and_asks_for_a_read_on_attaching() {
    let mut link = session();
    assert!(link.step(TradeEvent::Tick, 0).contains(&TradeAction::Connect));
    assert!(link.step(TradeEvent::Connected, 1).contains(&TradeAction::Authenticate));
    let listen = link.step(TradeEvent::Inbound(TradeMessage::Authorized), 2);
    assert!(listen.contains(&TradeAction::Send(listen_frame())));
    let live = link.step(TradeEvent::Inbound(TradeMessage::Listening), 3);
    assert_eq!(live[0], TradeAction::Report(TradeState::Live));
    assert!(live.contains(&TradeAction::Changed));
    assert!(
        link.step(TradeEvent::Inbound(TradeMessage::Reported), 4).contains(&TradeAction::Changed)
    );
}

/// A refused key lapses the link and it retries later. Silence after attaching does the same.
#[test]
fn a_refusal_or_silence_lapses_the_link() {
    let mut link = session();
    link.step(TradeEvent::Tick, 0);
    link.step(TradeEvent::Connected, 1);
    let refused = link.step(TradeEvent::Inbound(TradeMessage::Unauthorized), 2);
    assert!(refused.contains(&TradeAction::Disconnect));
    assert_eq!(link.state(), TradeState::Lapsed);

    let retry = link.wake_at().expect("a retry");
    link.step(TradeEvent::Tick, retry);
    link.step(TradeEvent::Connected, retry);
    link.step(TradeEvent::Inbound(TradeMessage::Authorized), retry);
    link.step(TradeEvent::Inbound(TradeMessage::Listening), retry);
    assert!(link.step(TradeEvent::Tick, retry + PING_INTERVAL_NS).contains(&TradeAction::Ping));
    let silent = link.step(TradeEvent::Tick, retry + LINK_SILENT_NS);
    assert!(silent.contains(&TradeAction::Disconnect));
    assert_eq!(link.state(), TradeState::Lapsed);
}

/// An unanswered authentication is given up after the answer bound.
#[test]
fn an_unanswered_authentication_is_given_up() {
    let mut link = session();
    link.step(TradeEvent::Tick, 0);
    link.step(TradeEvent::Connected, 0);
    let late = link.step(TradeEvent::Tick, ANSWER_TIMEOUT_NS);
    assert!(late.contains(&TradeAction::Disconnect));
}
