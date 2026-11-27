// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The stream's documented frames read, the link's state machine on injected events, and the
//! records a quote and a trade become.

use bijective_venue::contract::market::BookSnapshot;
use bijective_venue::contract::{ConnectionState, MarketFact, MarketFeed};
use bijective_venue::normalise::Scale;
use bijective_venue::resilience::SplitMix64;

use super::feed::Feed;
use super::message::{MarketMessage, auth_frame, parse};
use super::session::{
    ANSWER_TIMEOUT_NS, LINK_SILENT_NS, LinkAction, LinkEvent, LinkSession, LinkState,
    PING_INTERVAL_NS,
};

fn scales() -> (Scale, Scale) {
    (Scale::new(4).expect("price"), Scale::new(0).expect("quantity"))
}

/// The documented quote and trade examples, numbers as the venue writes them.
const QUOTE: &str = r#"[{"T":"q","S":"AAPL","bx":"V","bp":339.11,"bs":40,"ax":"V","ap":375.22,"as":40,"c":["R"],"t":"2026-11-16T21:00:00.030707912Z","z":"C"}]"#;
const TRADE: &str = r#"[{"T":"t","S":"AAPL","i":22606,"x":"V","p":352.66,"s":40,"c":["@","T"],"t":"2026-11-16T21:00:24.14502217Z","z":"C"}]"#;

#[test]
fn the_documented_frames_are_read_exactly() {
    let (price, quantity) = scales();
    let quote = parse(QUOTE.as_bytes(), price, quantity).expect("quote");
    let [MarketMessage::Quote { symbol, bid, ask, at_ns }] = quote.as_slice() else {
        panic!("{quote:?}")
    };
    assert_eq!(symbol, "AAPL");
    assert_eq!((bid.price_scaled, bid.quantity_scaled), (3_391_100, 40));
    assert_eq!((ask.price_scaled, ask.quantity_scaled), (3_752_200, 40));
    assert_eq!(at_ns.map(|n| n % 1_000_000_000), Some(30_707_912));

    let trade = parse(TRADE.as_bytes(), price, quantity).expect("trade");
    let [MarketMessage::Trade { trade_id, price_scaled, quantity_scaled, .. }] = trade.as_slice()
    else {
        panic!("{trade:?}")
    };
    assert_eq!((trade_id.as_str(), *price_scaled, *quantity_scaled), ("22606", 3_526_600, 40));

    let control = br#"[{"T":"success","msg":"connected"},{"T":"success","msg":"authenticated"},{"T":"subscription","trades":["AAPL"],"quotes":["AAPL"]},{"T":"error","code":406,"msg":"connection limit exceeded"}]"#;
    let read = parse(control, price, quantity).expect("control");
    assert_eq!(read[0], MarketMessage::Welcome);
    assert_eq!(read[1], MarketMessage::Authenticated);
    assert_eq!(read[2], MarketMessage::Subscribed);
    assert!(matches!(read[3], MarketMessage::Error { code: 406, .. }));
}

#[test]
fn a_price_finer_than_the_scale_is_refused_never_rounded() {
    let (price, quantity) = scales();
    assert!(parse(QUOTE.replace("339.11", "339.111111").as_bytes(), price, quantity).is_err());
    assert!(parse(b"{}", price, quantity).is_err(), "a frame is a list");
}

#[test]
fn a_quote_is_a_one_level_snapshot_and_a_one_sided_quote_has_no_top() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    let offered = feed.offer(QUOTE.as_bytes(), 1).expect("quote");
    let MarketFact::BookSnapshot(BookSnapshot { bids, asks, .. }) = &offered.records[0].fact else {
        panic!("a snapshot")
    };
    assert_eq!((bids.len(), asks.len()), (1, 1));
    let top = offered.top.expect("both sides");
    assert_eq!((top.bid_price, top.ask_price), (3_391_100, 3_752_200));

    let one_sided = QUOTE.replace(r#""ap":375.22,"as":40"#, r#""ap":0,"as":0"#);
    let offered = feed.offer(one_sided.as_bytes(), 2).expect("quote");
    assert!(offered.top.is_none(), "an empty side is absent, never zero");
    let traded = feed.offer(TRADE.as_bytes(), 3).expect("trade");
    assert!(matches!(traded.records[0].fact, MarketFact::Trade(_)));
    assert_eq!(feed.accepted(), 3);
}

fn session() -> LinkSession {
    LinkSession::new(
        auth_frame("key", "secret"),
        vec!["AAPL".into()],
        0,
        Box::new(SplitMix64::seeded(1)),
    )
}

fn sent(actions: &[LinkAction]) -> Vec<&String> {
    actions
        .iter()
        .filter_map(|a| if let LinkAction::Send(f) = a { Some(f) } else { None })
        .collect()
}

/// Authenticate on connect, subscribe on the venue's answer, report the link once subscribed.
#[test]
fn the_link_authenticates_then_subscribes_then_reports_itself_connected() {
    let mut link = session();
    assert!(link.step(LinkEvent::Tick, 0).contains(&LinkAction::Connect));
    let auth = link.step(LinkEvent::Connected, 1);
    assert!(sent(&auth)[0].contains(r#""action":"auth""#));
    let subscribe = link.step(LinkEvent::Inbound(&[MarketMessage::Authenticated]), 2);
    assert!(sent(&subscribe)[0].contains(r#""quotes":["AAPL"]"#));
    let live = link.step(LinkEvent::Inbound(&[MarketMessage::Subscribed]), 3);
    assert!(matches!(live[0], LinkAction::Report(ConnectionState::Connected { .. })));
    assert_eq!(link.state(), LinkState::Live);
}

/// The venue's refusal (a wrong key, its one-connection limit) lapses the link and it retries
/// later. An unanswered authentication does the same.
#[test]
fn a_refusal_or_silence_lapses_the_link_and_it_retries() {
    let mut link = session();
    link.step(LinkEvent::Tick, 0);
    link.step(LinkEvent::Connected, 1);
    let refused = MarketMessage::Error { code: 406, message: "connection limit exceeded".into() };
    let lapse = link.step(LinkEvent::Inbound(&[refused]), 2);
    assert!(lapse.contains(&LinkAction::Disconnect));
    assert!(lapse.contains(&LinkAction::Report(ConnectionState::Disconnected)));
    assert_eq!(link.state(), LinkState::Lapsed);
    assert_eq!(link.refusal.as_deref(), Some("406 connection limit exceeded"));

    let retry = link.wake_at().expect("a retry");
    link.step(LinkEvent::Tick, retry);
    link.step(LinkEvent::Connected, retry + 1);
    let unanswered = link.step(LinkEvent::Tick, retry + 1 + ANSWER_TIMEOUT_NS);
    assert!(unanswered.contains(&LinkAction::Disconnect));
}

/// A quiet market sends nothing. Pings keep the link, and their pongs count as traffic.
#[test]
fn a_live_link_pings_and_lapses_only_when_nothing_answers() {
    let mut link = session();
    link.step(LinkEvent::Tick, 0);
    link.step(LinkEvent::Connected, 0);
    link.step(LinkEvent::Inbound(&[MarketMessage::Authenticated]), 0);
    link.step(LinkEvent::Inbound(&[MarketMessage::Subscribed]), 0);

    assert!(link.step(LinkEvent::Tick, PING_INTERVAL_NS).contains(&LinkAction::Ping));
    link.step(LinkEvent::Pong, PING_INTERVAL_NS + 1);
    assert_eq!(link.state(), LinkState::Live);
    let silent = link.step(LinkEvent::Tick, PING_INTERVAL_NS + 1 + LINK_SILENT_NS);
    assert!(silent.contains(&LinkAction::Disconnect));
    assert_eq!(link.state(), LinkState::Lapsed);
}
