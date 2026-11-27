// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Gates for the book, the session, signing, execution and observation.

use bijective_venue::contract::Continuity;

use super::*;

/// Every symbol the fixtures name, at one pair. A symbol left out would read at the
/// outside-catalogue scale (decision 0007).
fn at(price: u8, quantity: u8) -> bijective_venue::profile::InstrumentScales {
    let pair = bijective_venue::profile::Scales {
        price: bijective_venue::normalise::Scale::new(price).unwrap(),
        quantity: bijective_venue::normalise::Scale::new(quantity).unwrap(),
    };
    bijective_venue::profile::InstrumentScales::uniform_over(
        [
            "AAAUSDT",
            "BTCUSDT",
            "ETHUSDT",
            "ROSEUSDT",
            "BTCUSD",
            "ETHPERP",
            "ETH-30DEC22-1400-C",
            "A",
            "S",
        ],
        pair,
    )
}

fn lvl(price: i128, qty: i128) -> Level {
    Level { price_scaled: price, quantity_scaled: qty }
}

fn primed() -> L2Book {
    let mut b = L2Book::new();
    b.apply_snapshot(
        &[lvl(2_654_300, 500), lvl(2_654_250, 900)],
        &[lvl(2_654_400, 300), lvl(2_654_450, 700)],
        100,
    );
    b
}

/// Checks `seq > last`, not `last + 1`: the sequence is per connection and jumps between one symbol's deltas.
#[test]
fn a_sequence_that_jumps_forward_is_healthy_not_a_gap() {
    let mut b = primed();
    assert!(b.apply_delta(&[lvl(2_654_300, 600)], &[], 107).is_ok());
    assert_eq!(b.best_bid(), Some((2_654_300, 600)));

    let consecutive_would_have_passed = 107 == 100 + 1;
    assert!(
        !consecutive_would_have_passed,
        "if the venue numbered consecutively this test would prove nothing"
    );
}

#[test]
fn a_snapshot_replaces_the_book_and_orders_both_sides() {
    let b = primed();
    assert_eq!(b.best_bid(), Some((2_654_300, 500)), "best bid is the highest");
    assert_eq!(b.best_ask(), Some((2_654_400, 300)), "best ask is the lowest");
    assert_eq!(b.depth(), (2, 2));
}

/// Zero means remove the level, not "a level with no size".
#[test]
fn a_zero_quantity_removes_the_level() {
    let mut b = primed();
    b.apply_delta(&[lvl(2_654_300, 0)], &[], 101).unwrap();
    assert_eq!(b.best_bid(), Some((2_654_250, 900)));
    assert_eq!(b.depth(), (1, 2));
}

#[test]
fn a_delta_before_any_snapshot_is_refused() {
    let mut b = L2Book::new();
    assert_eq!(b.apply_delta(&[lvl(1, 1)], &[], 5).unwrap_err(), BookError::NotInitialised);
}

/// A repeated sequence and a reversed one are different findings with the same policy.
#[test]
fn a_repeated_sequence_is_named_separately_from_a_reversed_one() {
    let mut b = primed();
    assert_eq!(
        b.apply_delta(&[], &[], 100).unwrap_err(),
        BookError::DuplicateSequence { seq: 100 }
    );
    assert!(!b.is_initialised(), "a book that took a bad delta is not a book");

    let mut c = primed();
    assert_eq!(
        c.apply_delta(&[], &[], 99).unwrap_err(),
        BookError::SequenceWentBackwards { last: 100, received: 99 }
    );
    assert!(!c.is_initialised());
}

/// After a loss the extent is unknown by construction, and the reconnect must say so.
#[test]
fn a_lost_book_declares_a_gap_of_unknown_extent() {
    let mut b = primed();
    assert_eq!(b.continuity_after_loss(), Continuity::Unbroken);

    let _ = b.apply_delta(&[], &[], 99);
    assert_eq!(b.continuity_after_loss(), Continuity::GapOfUnknownExtent);
    assert!(!b.continuity_after_loss().extent_is_known());
}

/// A crossed book is reported rather than acted on. The next delta may repair it.
#[test]
fn a_crossed_book_is_reported_and_survives() {
    let mut b = primed();
    assert!(!b.is_crossed());

    b.apply_delta(&[lvl(2_654_500, 100)], &[], 101).unwrap();
    assert!(b.is_crossed(), "a bid above the best ask did not read as crossed");
    assert!(b.is_initialised(), "reporting a cross must not discard the book");
}

#[test]
fn an_empty_side_is_not_crossed() {
    let mut b = L2Book::new();
    b.apply_snapshot(&[lvl(100, 1)], &[], 1);
    assert!(!b.is_crossed());
    assert_eq!(b.best_ask(), None);
}

#[test]
fn a_resubscribe_restores_the_book() {
    let mut b = primed();
    let _ = b.apply_delta(&[], &[], 99);
    assert!(!b.is_initialised());

    b.apply_snapshot(&[lvl(2_654_300, 500)], &[lvl(2_654_400, 300)], 200);
    assert!(b.is_initialised());
    assert!(b.apply_delta(&[lvl(2_654_300, 700)], &[], 205).is_ok());
    assert_eq!(b.best_bid(), Some((2_654_300, 700)));
}

#[test]
fn negative_and_wide_prices_order_correctly() {
    let mut b = L2Book::new();
    b.apply_snapshot(&[lvl(-500, 1), lvl(-100, 2)], &[lvl(i128::MAX / 2, 3), lvl(1_000, 4)], 1);
    assert_eq!(b.best_bid(), Some((-100, 2)));
    assert_eq!(b.best_ask(), Some((1_000, 4)));
}

use bijective_venue::normalise::{DecimalError, Scale};

use crate::message::{BookUpdateKind, ParseError, VenueMessage, parse};

fn scales() -> bijective_venue::profile::InstrumentScales {
    at(2, 3)
}

const SNAPSHOT: &str = r#"{"topic":"orderbook.50.AAAUSDT","type":"snapshot","ts":1700000000123,
  "data":{"s":"AAAUSDT","b":[["26543.00","0.500"],["26542.50","0.900"]],
  "a":[["26544.00","0.300"]],"seq":100}}"#;

#[test]
fn a_snapshot_parses_into_scaled_levels() {
    let s = scales();
    match parse(SNAPSHOT, &s).unwrap() {
        VenueMessage::Book { symbol, kind, bids, asks, seq, ts_ms } => {
            assert_eq!(symbol, "AAAUSDT");
            assert_eq!(kind, BookUpdateKind::Snapshot);
            assert_eq!(seq, 100);
            assert_eq!(ts_ms, 1_700_000_000_123);
            assert_eq!(bids[0], lvl(2_654_300, 500));
            assert_eq!(asks[0], lvl(2_654_400, 300));
        }
        other => panic!("expected a book message, got {other:?}"),
    }
}

/// An unreadable level is refused, not dropped.
#[test]
fn an_unreadable_level_is_refused_rather_than_filtered_out() {
    let s = scales();
    let text = r#"{"topic":"orderbook.50.AAAUSDT","type":"delta","ts":1,
      "data":{"s":"AAAUSDT","b":[["26543.00","0.500"],["26x43.00","0.100"]],"a":[],"seq":101}}"#;
    match parse(text, &s).unwrap_err() {
        ParseError::Level { side, index, source } => {
            assert_eq!(side, "bid");
            assert_eq!(index, 1, "the refusal names which level");
            assert!(matches!(source, DecimalError::NotANumber { found: 'x', .. }));
        }
        other => panic!("expected a level refusal, got {other:?}"),
    }
}

/// Garbage containing an `e` is refused as malformed, not as a bad character.
#[test]
fn garbage_containing_an_exponent_marker_is_still_refused() {
    let s = scales();
    let text = r#"{"topic":"orderbook.50.AAAUSDT","type":"delta","ts":1,
      "data":{"s":"AAAUSDT","b":[["not-a-price","0.100"]],"a":[],"seq":101}}"#;
    assert!(matches!(
        parse(text, &s).unwrap_err(),
        ParseError::Level { source: DecimalError::Malformed, index: 0, .. }
    ));
}

/// Precision the profile does not declare is refused here too, and names the profile defect.
#[test]
fn a_level_finer_than_the_profile_declares_is_refused() {
    let s = scales();
    let text = r#"{"topic":"orderbook.50.AAAUSDT","type":"delta","ts":1,
      "data":{"s":"AAAUSDT","b":[["26543.005","0.500"]],"a":[],"seq":101}}"#;
    assert!(matches!(
        parse(text, &s).unwrap_err(),
        ParseError::Level { source: DecimalError::ExcessPrecision { .. }, .. }
    ));
}

#[test]
fn a_subscribe_result_is_its_own_message() {
    let s = scales();
    let ok = r#"{"op":"subscribe","success":true,"ret_msg":""}"#;
    assert_eq!(
        parse(ok, &s).unwrap(),
        VenueMessage::SubscribeResult { op: "subscribe".into(), success: true }
    );

    let refused = r#"{"op":"subscribe","success":false,"ret_msg":"bad topic"}"#;
    match parse(refused, &s).unwrap() {
        VenueMessage::SubscribeResult { success, .. } => assert!(!success),
        other => panic!("expected a subscribe result, got {other:?}"),
    }
}

#[test]
fn a_pong_is_recognised() {
    let s = scales();
    assert_eq!(parse(r#"{"op":"pong","success":true}"#, &s).unwrap(), VenueMessage::Pong);
}

#[test]
fn an_unrecognised_topic_is_named() {
    let s = scales();
    let text = r#"{"topic":"kline.1.AAAUSDT","type":"snapshot","ts":1,"data":[]}"#;
    assert_eq!(
        parse(text, &s).unwrap(),
        VenueMessage::Unrecognised { topic: Some("kline.1.AAAUSDT".into()) }
    );
}

#[test]
fn a_book_message_missing_a_required_field_is_refused() {
    let s = scales();
    let no_type =
        r#"{"topic":"orderbook.50.AAAUSDT","ts":1,"data":{"s":"A","b":[],"a":[],"seq":1}}"#;
    assert_eq!(parse(no_type, &s).unwrap_err(), ParseError::MissingField { name: "type" });

    let no_ts =
        r#"{"topic":"orderbook.50.AAAUSDT","type":"delta","data":{"s":"A","b":[],"a":[],"seq":1}}"#;
    assert_eq!(parse(no_ts, &s).unwrap_err(), ParseError::MissingField { name: "ts" });

    assert_eq!(parse("not json at all", &s).unwrap_err(), ParseError::NotJson);
}

#[test]
fn an_unknown_book_type_is_refused_rather_than_assumed_to_be_a_delta() {
    let s = scales();
    let text = r#"{"topic":"orderbook.50.A","type":"reset","ts":1,"data":{"s":"A","b":[],"a":[],"seq":1}}"#;
    assert!(matches!(parse(text, &s).unwrap_err(), ParseError::UnknownBookType { .. }));
}

/// Parser and book together: the venue's own message shapes drive the lifted sequence rule.
#[test]
fn a_snapshot_then_a_jumping_delta_maintains_the_book() {
    let s = scales();
    let mut book = L2Book::new();

    let VenueMessage::Book { bids, asks, seq, .. } = parse(SNAPSHOT, &s).unwrap() else {
        panic!("expected a book message");
    };
    book.apply_snapshot(&bids, &asks, seq);
    assert_eq!(book.best_bid(), Some((2_654_300, 500)));

    // seq jumps 100 -> 107, which is healthy on a per-connection counter.
    let delta = r#"{"topic":"orderbook.50.AAAUSDT","type":"delta","ts":2,
      "data":{"s":"AAAUSDT","b":[["26543.00","0"]],"a":[],"seq":107}}"#;
    let VenueMessage::Book { bids, asks, seq, kind, .. } = parse(delta, &s).unwrap() else {
        panic!("expected a book message");
    };
    assert_eq!(kind, BookUpdateKind::Delta);
    book.apply_delta(&bids, &asks, seq).unwrap();

    assert_eq!(book.best_bid(), Some((2_654_250, 900)), "the zero removed the top level");
}

use crate::message::PING_INTERVAL_NS;
use crate::session::{
    Action, CONNECT_TIMEOUT_NS, ConnectionState, Degraded, Event, Failure, INBOUND_IDLE_LIMIT_NS,
    MAX_RECONNECT_ATTEMPTS, SUBSCRIBE_ACK_TIMEOUT_NS, Session, SessionError,
};

const T0: i128 = 1_700_000_000_000_000_000;

fn topics() -> Vec<String> {
    vec!["orderbook.50.BTCUSDT".to_string()]
}

fn live_at(t: i128) -> (Session, i128) {
    let mut s = Session::new(topics(), t).unwrap();
    s.step(Event::Tick, t);
    s.step(Event::Connected, t);
    let ack = VenueMessage::SubscribeResult { op: "subscribe".into(), success: true };
    s.step(Event::Inbound(&ack), t);
    assert_eq!(*s.state(), ConnectionState::Live);
    (s, t)
}

#[test]
fn a_session_with_no_topics_is_refused() {
    assert_eq!(Session::new(vec![], T0).unwrap_err(), SessionError::NoTopics);
}

#[test]
fn more_topics_than_a_connection_carries_is_refused() {
    let many: Vec<String> = (0..11).map(|i| format!("orderbook.50.S{i}")).collect();
    assert!(matches!(
        Session::new(many, T0).unwrap_err(),
        SessionError::TooManyTopics { asked: 11, limit: 10 }
    ));
}

#[test]
fn a_down_session_asks_to_connect_and_gives_the_attempt_a_deadline() {
    let mut s = Session::new(topics(), T0).unwrap();
    let acts = s.step(Event::Tick, T0);
    assert!(acts.contains(&Action::Connect));
    assert_eq!(*s.state(), ConnectionState::Connecting);
    assert_eq!(s.wake_at(), Some(T0 + CONNECT_TIMEOUT_NS));
}

#[test]
fn a_connect_that_never_answers_is_abandoned_not_awaited() {
    let mut s = Session::new(topics(), T0).unwrap();
    s.step(Event::Tick, T0);
    let acts = s.step(Event::Tick, T0 + CONNECT_TIMEOUT_NS);
    assert!(acts.contains(&Action::Disconnect));
    assert_eq!(*s.state(), ConnectionState::Degraded(Degraded::ConnectTimedOut));
}

#[test]
fn a_socket_is_not_a_subscription() {
    let mut s = Session::new(topics(), T0).unwrap();
    s.step(Event::Tick, T0);
    let acts = s.step(Event::Connected, T0);

    assert_eq!(*s.state(), ConnectionState::Subscribing);
    assert!(acts.iter().any(|a| matches!(a, Action::Send(f) if f.contains("subscribe"))));
    assert!(!acts.contains(&Action::Report(ConnectionState::Live)));
}

/// The confirmed-subscription rule: book traffic before confirmation must not count as confirmation.
#[test]
fn book_traffic_before_the_acknowledgement_does_not_confirm_the_subscription() {
    let mut s = Session::new(topics(), T0).unwrap();
    s.step(Event::Tick, T0);
    s.step(Event::Connected, T0);

    let book = VenueMessage::Book {
        symbol: "BTCUSDT".into(),
        kind: BookUpdateKind::Snapshot,
        bids: vec![],
        asks: vec![],
        seq: 1,
        ts_ms: 0,
    };
    s.step(Event::Inbound(&book), T0 + 1_000);

    assert_eq!(*s.state(), ConnectionState::Subscribing);
}

#[test]
fn a_subscribe_the_venue_never_answers_is_a_bad_connection() {
    let mut s = Session::new(topics(), T0).unwrap();
    s.step(Event::Tick, T0);
    s.step(Event::Connected, T0);

    let acts = s.step(Event::Tick, T0 + SUBSCRIBE_ACK_TIMEOUT_NS);
    assert!(acts.contains(&Action::Disconnect));
    assert_eq!(*s.state(), ConnectionState::Degraded(Degraded::SubscribeUnanswered));
}

#[test]
fn a_refused_subscribe_is_not_retried_on_the_same_socket() {
    let mut s = Session::new(topics(), T0).unwrap();
    s.step(Event::Tick, T0);
    s.step(Event::Connected, T0);

    let no = VenueMessage::SubscribeResult { op: "subscribe".into(), success: false };
    let acts = s.step(Event::Inbound(&no), T0 + 10);
    assert!(acts.contains(&Action::Disconnect));
    assert_eq!(*s.state(), ConnectionState::Degraded(Degraded::SubscribeRefused));
}

#[test]
fn the_acknowledgement_is_what_makes_it_live() {
    let (s, _) = live_at(T0);
    assert_eq!(*s.state(), ConnectionState::Live);
}

#[test]
fn a_live_link_pings_on_the_interval_and_not_before() {
    let (mut s, t) = live_at(T0);

    let early = s.step(Event::Tick, t + PING_INTERVAL_NS - 1);
    assert!(!early.iter().any(|a| matches!(a, Action::Send(f) if f.contains("ping"))));

    let due = s.step(Event::Tick, t + PING_INTERVAL_NS);
    assert!(due.iter().any(|a| matches!(a, Action::Send(f) if f.contains("ping"))));
}

/// Half-open link: socket open, venue silent. The adapter must not report healthy.
#[test]
fn a_silent_link_is_dead_even_though_the_socket_is_open() {
    let (mut s, t) = live_at(T0);
    let acts = s.step(Event::Tick, t + INBOUND_IDLE_LIMIT_NS);

    assert!(acts.contains(&Action::Disconnect));
    assert!(acts.contains(&Action::ResetBooks));
    assert_eq!(*s.state(), ConnectionState::Degraded(Degraded::LinkSilent));
}

#[test]
fn a_pong_keeps_the_link_alive() {
    let (mut s, t) = live_at(T0);
    s.step(Event::Inbound(&VenueMessage::Pong), t + PING_INTERVAL_NS);

    s.step(Event::Tick, t + INBOUND_IDLE_LIMIT_NS);
    assert_eq!(*s.state(), ConnectionState::Live);
}

/// The adapter contract: never reconnect into a gap silently. Books go and the transition is reported.
#[test]
fn a_sequence_gap_drops_the_books_and_says_so() {
    let (mut s, t) = live_at(T0);
    let acts = s.step(Event::SequenceGap, t + 5_000);

    assert!(acts.contains(&Action::ResetBooks));
    assert!(acts.contains(&Action::Report(ConnectionState::Degraded(Degraded::SequenceGap))));
    assert_eq!(*s.state(), ConnectionState::Degraded(Degraded::SequenceGap));
}

/// The driver acts on the list in order, so the report must come first.
#[test]
fn a_transition_is_reported_before_the_action_that_supersedes_it() {
    let mut s = Session::new(topics(), T0).unwrap();
    let acts = s.step(Event::Tick, T0);

    let report = acts.iter().position(|a| matches!(a, Action::Report(_)));
    let connect = acts.iter().position(|a| matches!(a, Action::Connect));
    assert!(report < connect, "the state is announced before the connection is attempted");
}

#[test]
fn a_state_that_did_not_change_is_not_reported() {
    let (mut s, t) = live_at(T0);
    let quiet = s.step(Event::Tick, t + 1_000);
    assert!(!quiet.iter().any(|a| matches!(a, Action::Report(_))));
}

#[test]
fn the_backoff_grows_and_its_ceiling_actually_binds() {
    let mut s = Session::new(topics(), T0).unwrap();
    let mut t = T0;
    let mut waits = Vec::new();

    for _ in 0..12 {
        s.step(Event::Tick, t);
        s.step(Event::ConnectFailed, t);
        let due = s.wake_at().expect("a retry is scheduled");
        waits.push(due - t);
        t = due;
    }

    assert!(waits[1] > waits[0], "backoff grows");
    assert_eq!(
        *waits.last().unwrap(),
        300_000_000_000,
        "the ceiling is reached, not merely written down"
    );
}

#[test]
fn a_link_that_will_not_come_back_stops_trying_and_reports_it() {
    let mut s = Session::new(topics(), T0).unwrap();
    let mut t = T0;

    for _ in 0..MAX_RECONNECT_ATTEMPTS + 1 {
        s.step(Event::Tick, t);
        s.step(Event::ConnectFailed, t);
        t = s.wake_at().unwrap_or(t + 1_000_000_000);
    }

    assert_eq!(*s.state(), ConnectionState::Failed(Failure::ReconnectAttemptsExhausted));
    assert_eq!(s.wake_at(), None, "a failed session schedules nothing");
}

#[test]
fn a_successful_connect_clears_the_attempt_count() {
    let mut s = Session::new(topics(), T0).unwrap();
    let mut t = T0;
    for _ in 0..3 {
        s.step(Event::Tick, t);
        s.step(Event::ConnectFailed, t);
        t = s.wake_at().unwrap();
    }
    s.step(Event::Tick, t);
    s.step(Event::Connected, t);
    s.step(Event::SocketClosed, t + 1_000);

    // One failure after a good connect must back off like a first failure, not like a fourth.
    let first_wait = s.wake_at().unwrap() - (t + 1_000);
    assert_eq!(first_wait, 6_000_000_000);
}

use crate::feed::Feed;
use bijective_venue::contract::{FeedError, MarketFeed};

fn snapshot_frame(symbol: &str, seq: u64) -> String {
    format!(
        r#"{{"topic":"orderbook.50.{symbol}","type":"snapshot","ts":1787469688129,"data":{{"s":"{symbol}","b":[["100.00","1.000"]],"a":[["101.00","2.000"]],"seq":{seq},"u":{seq}}}}}"#
    )
}

#[test]
fn a_snapshot_normalises_into_a_book_top() {
    let mut f = Feed::new(at(2, 3));
    let top = f.offer(snapshot_frame("BTCUSDT", 10).as_bytes(), 7).unwrap().top.expect("a top");

    assert_eq!(top.instrument.as_str(), "BTCUSDT");
    assert_eq!(top.bid_price, 10_000);
    assert_eq!(top.ask_price, 10_100);
    assert_eq!(f.accepted(), 1);
}

/// A delta with no snapshot is refused. Applying it would report a top that was never true.
#[test]
fn an_update_before_a_snapshot_is_refused() {
    let mut f = Feed::new(at(2, 3));
    let delta = snapshot_frame("BTCUSDT", 5).replace("snapshot", "delta");

    assert_eq!(
        f.offer(delta.as_bytes(), 7).unwrap_err(),
        FeedError::NoSnapshot { instrument: "BTCUSDT".into() }
    );
}

#[test]
fn an_unreadable_frame_is_refused_not_dropped_by_the_feed() {
    let mut f = Feed::new(at(2, 3));
    assert!(matches!(f.offer(b"not json at all", 7), Err(FeedError::Unreadable { .. })));
    assert_eq!(f.accepted(), 0);
}

#[test]
fn a_frame_that_is_not_utf8_is_refused_rather_than_lossily_read() {
    let mut f = Feed::new(at(2, 3));
    assert!(matches!(f.offer(&[0xff, 0xfe, 0x00], 7), Err(FeedError::Unreadable { .. })));
}

#[test]
fn an_acknowledgement_carries_no_observation() {
    let mut f = Feed::new(at(2, 3));
    let ack = br#"{"success":true,"ret_msg":"","op":"subscribe","conn_id":"x"}"#;
    assert_eq!(f.offer(ack, 7).unwrap(), bijective_venue::contract::Offered::default());
}

#[test]
fn a_reset_forgets_every_book() {
    let mut f = Feed::new(at(2, 3));
    f.offer(snapshot_frame("BTCUSDT", 1).as_bytes(), 7).unwrap();
    assert_eq!(f.instruments(), 1);
    f.reset();
    assert_eq!(f.instruments(), 0);
}

use crate::sign::{RECV_WINDOW_MS, sign, signing_payload};

/// RFC 4231 test case 1.
#[test]
fn hmac_sha256_matches_the_published_vector() {
    let key = [0x0bu8; 20];
    let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::KeyInit>::new_from_slice(&key).unwrap();
    hmac::Mac::update(&mut mac, b"Hi There");
    let digest = hmac::Mac::finalize(mac).into_bytes();

    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
}

/// RFC 4231 test case 2, with a key shorter than the block size.
#[test]
fn hmac_sha256_matches_the_published_vector_for_a_short_key() {
    let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::KeyInit>::new_from_slice(b"Jefe").unwrap();
    hmac::Mac::update(&mut mac, b"what do ya want for nothing?");
    let digest = hmac::Mac::finalize(mac).into_bytes();

    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
}

/// The ordering is the half that gets signing wrong, and it is checkable without a secret.
#[test]
fn the_signing_payload_concatenates_in_the_venues_order() {
    assert_eq!(
        signing_payload(1_700_000_000_000, "a-key", r#"{"symbol":"X"}"#),
        format!("1700000000000a-key{RECV_WINDOW_MS}{{\"symbol\":\"X\"}}")
    );
}

#[test]
fn a_signature_is_lowercase_hex_of_the_expected_width() {
    let signature = sign(b"a-secret", 1_700_000_000_000, "a-key", "");

    assert_eq!(signature.len(), 64, "sha-256 is 32 bytes");
    assert!(signature.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

/// Every input is part of the signature. An omitted field is one an attacker may change.
#[test]
fn changing_any_input_changes_the_signature() {
    let base = sign(b"secret", 1_700_000_000_000, "key", "body");

    assert_ne!(base, sign(b"other", 1_700_000_000_000, "key", "body"), "secret");
    assert_ne!(base, sign(b"secret", 1_700_000_000_001, "key", "body"), "timestamp");
    assert_ne!(base, sign(b"secret", 1_700_000_000_000, "other", "body"), "api key");
    assert_ne!(base, sign(b"secret", 1_700_000_000_000, "key", "other"), "body");
}

/// Same inputs sign the same way, so an idempotent resubmission is not a different request.
#[test]
fn signing_is_deterministic() {
    let a = sign(b"secret", 1_700_000_000_000, "key", "body");
    let b = sign(b"secret", 1_700_000_000_000, "key", "body");
    assert_eq!(a, b);
}

use crate::execution::{RequestError, classify, order_body, read_response};
use bijective_venue::contract::{OrderRequest, Side};
use bijective_venue::normalise::{Attempted, Rejection, Unknown, VenueOutcome};

fn order() -> OrderRequest {
    OrderRequest {
        client_id: "bl-0001".into(),
        instrument: "BTCUSDT".into(),
        side: Side::Buy,
        quantity_scaled: 1_500,
        price_scaled: Some(7_625_350),
        reduce_only: false,
        terms: Default::default(),
    }
}

#[test]
fn a_limit_order_renders_its_numbers_exactly() {
    let (p, q) = (Scale::new(2).unwrap(), Scale::new(3).unwrap());
    let body: serde_json::Value =
        serde_json::from_str(&order_body(&order(), p, q).unwrap()).unwrap();

    assert_eq!(body["price"], "76253.50");
    assert_eq!(body["qty"], "1.500");
    assert_eq!(body["orderType"], "Limit");
    assert_eq!(body["side"], "Buy");
    assert_eq!(body["orderLinkId"], "bl-0001");
}

#[test]
fn an_order_with_no_price_is_a_market_order_and_carries_none() {
    let (p, q) = (Scale::new(2).unwrap(), Scale::new(3).unwrap());
    let mut o = order();
    o.price_scaled = None;
    let body: serde_json::Value = serde_json::from_str(&order_body(&o, p, q).unwrap()).unwrap();

    assert_eq!(body["orderType"], "Market");
    assert!(body.get("price").is_none(), "a market order must not carry a price");
}

#[test]
fn reduce_only_reaches_the_body() {
    let (p, q) = (Scale::new(2).unwrap(), Scale::new(3).unwrap());
    let mut o = order();
    o.reduce_only = true;
    let body: serde_json::Value = serde_json::from_str(&order_body(&o, p, q).unwrap()).unwrap();

    assert_eq!(body["reduceOnly"], true);
}

/// Refused here: a venue refusal costs a round trip and reads like a strategy rejection.
#[test]
fn an_order_missing_what_it_needs_is_refused_before_it_is_sent() {
    let (p, q) = (Scale::new(2).unwrap(), Scale::new(3).unwrap());

    let mut no_id = order();
    no_id.client_id = "  ".into();
    assert_eq!(order_body(&no_id, p, q).unwrap_err(), RequestError::EmptyClientId);

    let mut no_instrument = order();
    no_instrument.instrument = bijective_venue::profile::VenueSymbol::new("");
    assert_eq!(order_body(&no_instrument, p, q).unwrap_err(), RequestError::EmptyInstrument);

    let mut zero = order();
    zero.quantity_scaled = 0;
    assert_eq!(order_body(&zero, p, q).unwrap_err(), RequestError::NonPositiveQuantity);

    let mut negative = order();
    negative.quantity_scaled = -1;
    assert_eq!(order_body(&negative, p, q).unwrap_err(), RequestError::NonPositiveQuantity);
}

#[test]
fn the_venues_success_code_is_an_acceptance() {
    assert_eq!(classify(0, "OK"), VenueOutcome::Accepted);
}

/// The remedy is to resize, not reprice. `PriceOutOfBand` would retry into the same refusal.
#[test]
fn an_order_below_the_venues_minimum_is_classified_rather_than_carried_as_unknown() {
    assert_eq!(
        classify(110_094, "Order value exceeded lower limit"),
        VenueOutcome::Rejected(Rejection::BelowMinimumSize)
    );
}

/// Guessing the nearest known rejection is how a platform responds confidently and wrongly.
#[test]
fn an_unrecognised_code_travels_as_itself() {
    match classify(999_999, "something new") {
        VenueOutcome::Rejected(Rejection::Unclassified { venue_code }) => {
            assert!(venue_code.contains("999999"), "the code survives: {venue_code}");
            assert!(venue_code.contains("something new"), "and so does the venue's wording");
        }
        other => panic!("an unknown code was mapped to {other:?}"),
    }
}

/// The asymmetry that matters most. The order may have reached the venue and filled.
#[test]
fn an_unreadable_answer_is_unknown_and_never_a_rejection() {
    for body in ["not json", "{}", r#"{"retCode":"not a number"}"#, ""] {
        let outcome = read_response(body);
        assert_eq!(outcome, VenueOutcome::Unknown(Unknown::Unreadable), "for {body:?}");
        assert!(!outcome.is_known_not_to_have_happened());
        assert!(
            outcome.needs_reconciliation(Attempted::Submit),
            "only the venue's account can settle it"
        );
    }
}

#[test]
fn a_readable_answer_is_classified() {
    assert_eq!(read_response(r#"{"retCode":0,"retMsg":"OK"}"#), VenueOutcome::Accepted);
    assert_eq!(
        read_response(r#"{"retCode":110094,"retMsg":"too small"}"#),
        VenueOutcome::Rejected(Rejection::BelowMinimumSize)
    );
}

/// A rejection permits the platform to conclude the order did not happen. An unknown never does.
#[test]
fn only_a_rejection_permits_concluding_the_order_did_not_happen() {
    assert!(classify(110_094, "").is_known_not_to_have_happened());
    assert!(classify(999_999, "").is_known_not_to_have_happened());
    assert!(!read_response("garbage").is_known_not_to_have_happened());
}

/// A refused credential answers 401 with an empty body (observed). Body-only classification would be Unknown.
#[test]
fn a_refused_credential_is_a_rejection_and_not_an_unknown() {
    use crate::execution::read_http_response;

    let outcome = read_http_response(401, "");

    assert_eq!(outcome, VenueOutcome::Rejected(Rejection::CredentialRefused));
    assert!(outcome.is_known_not_to_have_happened(), "auth is checked before matching");
    assert!(
        !outcome.needs_reconciliation(Attempted::Submit),
        "a mistyped key is not capital movement"
    );
}

#[test]
fn rate_limiting_is_a_rejection_even_with_no_body() {
    use crate::execution::read_http_response;

    assert_eq!(
        read_http_response(429, ""),
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None })
    );
}

/// A gateway error may arrive after the venue accepted the order: that is what `Unknown` is for.
#[test]
fn a_server_error_stays_unknown() {
    use crate::execution::read_http_response;

    for status in [500, 502, 503, 504] {
        let outcome = read_http_response(status, "");
        assert_eq!(outcome, VenueOutcome::Unknown(Unknown::Unreadable), "for {status}");
        assert!(outcome.needs_reconciliation(Attempted::Submit));
    }
}

#[test]
fn a_success_status_is_still_classified_by_the_body() {
    use crate::execution::read_http_response;

    assert_eq!(read_http_response(200, r#"{"retCode":0,"retMsg":"OK"}"#), VenueOutcome::Accepted);
    assert_eq!(
        read_http_response(200, r#"{"retCode":110094,"retMsg":"too small"}"#),
        VenueOutcome::Rejected(Rejection::BelowMinimumSize)
    );
    // 200 says the request arrived, not what the venue did with it.
    assert_eq!(read_http_response(200, "garbage"), VenueOutcome::Unknown(Unknown::Unreadable));
}

/// A no-op amend would re-queue the order and cost book position. Refused before sending.
#[test]
fn an_amend_that_changes_nothing_is_refused_before_it_is_sent() {
    use crate::execution::RequestError;

    fn check(price: Option<i128>, qty: Option<i128>) -> Result<(), RequestError> {
        if price.is_none() && qty.is_none() {
            return Err(RequestError::AmendChangesNothing);
        }
        if qty.is_some_and(|q| q <= 0) {
            return Err(RequestError::NonPositiveQuantity);
        }
        Ok(())
    }

    assert_eq!(check(None, None).unwrap_err(), RequestError::AmendChangesNothing);
    assert_eq!(check(None, Some(0)).unwrap_err(), RequestError::NonPositiveQuantity);
    assert!(check(Some(1), None).is_ok());
    assert!(check(None, Some(1)).is_ok());
    assert!(check(Some(1), Some(1)).is_ok());
}

use crate::observe::{ObserveError, read_instrument};

/// Recorded venue answer, kept verbatim: a hand-written fixture would test our belief. The coin
/// fields are from the live answer read 2026-11-15.
const INSTRUMENT_ANSWER: &str = r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[
{"symbol":"BTCUSDT","contractType":"LinearPerpetual","status":"Trading","baseCoin":"BTC","quoteCoin":"USDT","settleCoin":"USDT","priceScale":"2",
 "lotSizeFilter":{"maxOrderQty":"1500.000","minOrderQty":"0.001","qtyStep":"0.001","minNotionalValue":"5"},
 "priceFilter":{"minPrice":"0.10","maxPrice":"1999999.80","tickSize":"0.10"},
 "fundingInterval":480}]}}"#;

#[test]
fn the_venues_published_rules_are_read_as_the_venues_own() {
    let observed = read_instrument(INSTRUMENT_ANSWER, "BTCUSDT").unwrap();
    let precision = observed.precision.expect("precision was published");

    assert_eq!(precision.rules.price_scale, 2);
    assert_eq!(precision.rules.quantity_scale, 3, "read from the venue's own step, not assumed");
    assert_eq!(precision.rules.price_tick, 10, "0.10 at scale 2");
    assert_eq!(precision.rules.quantity_lot, 1, "0.001 at scale 3");
}

/// Minutes on the wire, nanoseconds in the platform. Eight hours.
/// The asset a quantity counts, and what it is quoted and settled in, as the venue names them.
#[test]
fn the_venues_coins_are_read_and_a_row_without_one_is_refused() {
    let observed = read_instrument(INSTRUMENT_ANSWER, "BTCUSDT").expect("observed");
    let coins = observed.currencies.expect("published");
    assert_eq!(
        (coins.base.as_str(), coins.quote.as_deref(), coins.settlement.as_deref()),
        ("BTC", Some("USDT"), Some("USDT"))
    );

    let without = INSTRUMENT_ANSWER.replace(r#""baseCoin":"BTC","#, "");
    assert!(matches!(read_instrument(&without, "BTCUSDT"), Err(ObserveError::Unreadable { .. })));
}

#[test]
fn the_funding_interval_is_converted_and_not_carried_in_the_venues_units() {
    let observed = read_instrument(INSTRUMENT_ANSWER, "BTCUSDT").unwrap();
    let funding = observed.funding.get(&bijective_venue::profile::Transport::Stream).unwrap();

    assert_eq!(
        *funding,
        bijective_venue::profile::FundingModel::Published { interval_ns: 28_800_000_000_000 }
    );
}

/// An unreadable answer must not become a blank observation.
#[test]
fn an_unreadable_answer_is_refused_rather_than_returned_empty() {
    assert!(matches!(read_instrument("not json", "BTCUSDT"), Err(ObserveError::Unreadable { .. })));
    assert!(matches!(
        read_instrument(r#"{"retCode":10001,"retMsg":"bad request"}"#, "BTCUSDT"),
        Err(ObserveError::Unreadable { .. })
    ));
}

#[test]
fn an_instrument_the_venue_does_not_list_is_named() {
    match read_instrument(INSTRUMENT_ANSWER, "NOTLISTED") {
        Err(ObserveError::UnknownInstrument { instrument }) => assert_eq!(instrument, "NOTLISTED"),
        other => panic!("expected an unknown instrument, got {other:?}"),
    }
}

/// Increments are decimal strings. Inexact reading would make later tick checks measure a fiction.
#[test]
fn an_increment_that_cannot_be_read_exactly_is_refused() {
    let broken = INSTRUMENT_ANSWER.replace(r#""tickSize":"0.10""#, r#""tickSize":"0.1x""#);
    assert!(matches!(
        read_instrument(&broken, "BTCUSDT"),
        Err(ObserveError::Increment { field: "tickSize", .. })
    ));
}

/// An untracked figure must say so, not report zero: a zero instant reads as the epoch.
#[test]
fn the_session_reports_the_instants_it_does_not_keep_as_untracked() {
    use crate::session::{ConnectionState, Degraded};
    use bijective_venue::contract::{ConnectionState as Contract, DegradedReason, Moment};

    assert_eq!(
        crate::stream::translate(&ConnectionState::Live, 3),
        Contract::Connected { since: Moment::NotTracked }
    );

    assert_eq!(
        crate::stream::translate(&ConnectionState::Degraded(Degraded::LinkSilent), 3),
        Contract::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::Stale { last_message: Moment::NotTracked },
        }
    );
}

/// The count the session has travels rather than being invented.
#[test]
fn a_refused_subscription_reports_how_many_were_actually_asked_for() {
    use crate::session::{ConnectionState, Degraded};
    use bijective_venue::contract::{ConnectionState as Contract, DegradedReason, Moment};

    let state =
        crate::stream::translate(&ConnectionState::Degraded(Degraded::SubscribeRefused), 40);
    assert_eq!(
        state,
        Contract::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::SubscriptionsIncomplete { attached: 0, requested: 40 },
        }
    );
}

/// Matches what the venue publishes. The divergent case changes one field and watches it fire.
fn declared(symbol: &str) -> bijective_venue::profile::VenueProfile {
    use bijective_venue::profile::{
        DataClass, FundingModel, IdentifierRules, Instrument, InstrumentCatalogue, InstrumentId,
        InstrumentKind, InstrumentStatus, OrderTypeSupport, PrecisionRules, PriceValidity,
        ReduceOnly, SupportDepth, SupportEntry, Transport, TransportProfile, VenueProfile,
        VenueSymbol, declaration::PROFILE_FORMAT,
    };

    let mut transports = std::collections::BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Published { interval_ns: 28_800_000_000_000 },
            data_classes: vec![DataClass::TopOfBook],
            max_staleness_ns: Some(40_000_000_000),
        },
    );

    VenueProfile {
        format: PROFILE_FORMAT,
        venue: "bybit".into(),
        verified_at: None,
        transports,
        reduce_only: ReduceOnly::FillTimeClamp,
        order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
        instruments: InstrumentCatalogue::from_instruments([Instrument {
            symbol: VenueSymbol::new(symbol),
            id: InstrumentId::new(symbol),
            kind: InstrumentKind::Perpetual,
            settlement_currency: "usdt".into(),
            quote_currency: "usdt".into(),
            base_currency: Some("btc".into()),
            precision: PrecisionRules {
                price_scale: 2,
                quantity_scale: 3,
                price_tick: 10,
                quantity_lot: 1,
                price_validity: PriceValidity::IncrementOnly,
            },
            minimum_size: 1,
            minimum_notional: 500,
            status: InstrumentStatus::Trading,
        }]),
        identifier_rules: IdentifierRules {
            allowed: "abcdefghijklmnopqrstuvwxyz0123456789-".into(),
            max_len: 36,
        },
        transformations: Vec::new(),
        support: SupportEntry::declare("bybit", SupportDepth::Full, true).expect("declared"),
    }
}

fn store(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-runner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// The venue answer in, a conclusion computed, not entered by hand, recorded with the digest of its source.
#[test]
fn a_conclusion_is_drawn_from_the_venues_answer_and_recorded_with_it() {
    use bijective_venue_assurance::wire::{ASSURANCE, Concluded, Outcome, record, standing};
    use bijective_venue_assurance::{AssuranceReport, verify};

    let dir = store("confirmed");
    let observed = read_instrument(INSTRUMENT_ANSWER, "BTCUSDT").expect("observed");
    let report = AssuranceReport::new("bybit", verify(&declared("BTCUSDT"), &observed, 11));
    let outcome = Outcome::of(&report).drawn_from(INSTRUMENT_ANSWER.as_bytes());

    let mut stream =
        bijective_event_store::store::StreamWriter::open(&dir, ASSURANCE).expect("open");
    record(&outcome, &mut stream).expect("record");
    drop(stream);

    let stood = standing(&dir, "bybit").expect("read").expect("recorded");
    assert_eq!(
        stood.concluded,
        Concluded::Confirmed,
        "the profile declares what the venue publishes"
    );
    assert_eq!(
        stood.evidence,
        Some(bijective_event_store::digest::sha256(INSTRUMENT_ANSWER.as_bytes())),
        "the conclusion names the answer it was drawn from"
    );
}

/// One field wrong in the declaration, and the verifier says so rather than the runner.
#[test]
fn a_profile_that_disagrees_with_the_answer_is_recorded_as_diverged() {
    use bijective_venue_assurance::wire::{Concluded, Outcome};
    use bijective_venue_assurance::{AssuranceReport, verify};

    let mut profile = declared("BTCUSDT");
    // The venue publishes a tick of 0.10 at scale 2, which is 10.
    let instrument = profile.instruments.iter().next().expect("one").clone();
    profile.instruments = bijective_venue::profile::InstrumentCatalogue::from_instruments([
        bijective_venue::profile::Instrument {
            precision: bijective_venue::profile::PrecisionRules {
                price_tick: 1,
                ..instrument.precision
            },
            ..instrument
        },
    ]);

    let observed = read_instrument(INSTRUMENT_ANSWER, "BTCUSDT").expect("observed");
    let outcome = Outcome::of(&AssuranceReport::new("bybit", verify(&profile, &observed, 11)))
        .drawn_from(INSTRUMENT_ANSWER.as_bytes());

    match outcome.concluded {
        Concluded::Diverged { findings, capital_relevant } => {
            assert!(findings >= 1);
            assert!(capital_relevant, "a tick the venue does not enforce is capital-relevant");
        }
        other => panic!("a declaration that disagrees must not confirm: {other:?}"),
    }
}

/// The waits of a session that fails to connect `failures` times in a row, from one seed.
fn reconnect_waits(seed: u64, failures: u32) -> Vec<i128> {
    let mut s = Session::with_jitter(
        topics(),
        T0,
        Box::new(bijective_venue::resilience::SplitMix64::seeded(seed)),
    )
    .unwrap();
    let mut t = T0;
    let mut waits = Vec::new();
    for _ in 0..failures {
        s.step(Event::Tick, t);
        s.step(Event::ConnectFailed, t);
        let due = s.wake_at().expect("a retry is scheduled");
        waits.push(due - t);
        t = due;
    }
    waits
}

/// Clients that retry in step knock a recovering venue over again. Each wait is spread over the
/// upper half of its range and never goes below the lower half.
#[test]
fn a_reconnect_wait_is_between_half_the_longest_and_the_longest() {
    for seed in 0..20 {
        for (index, wait) in reconnect_waits(seed, 10).into_iter().enumerate() {
            let failures = u32::try_from(index).unwrap() + 1;
            let longest = (3_000_000_000i128 << failures).min(300_000_000_000);
            assert!(
                wait >= longest / 2 && wait <= longest,
                "seed {seed}, failure {failures}: {wait} of {longest}"
            );
        }
    }
}

#[test]
fn two_sessions_with_different_seeds_do_not_retry_in_step() {
    assert_eq!(reconnect_waits(1, 6), reconnect_waits(1, 6), "a seed fixes the waits");
    assert_ne!(reconnect_waits(1, 6), reconnect_waits(2, 6));
}

/// The venue's own page: 403 is "access too frequent" and means an IP block of at least ten
/// minutes, not a wrong key. Read as a refused credential it would never be retried.
#[test]
fn a_403_is_an_ip_block_that_says_to_wait_ten_minutes() {
    use crate::execution::read_http_response;

    let outcome = read_http_response(403, "");

    assert_eq!(
        outcome,
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(600_000_000_000) })
    );
    assert!(outcome.is_known_not_to_have_happened());
}

/// The venue's error-code page (read 2026-11-05): 110072 is "OrderLinkedID is duplicate". After a
/// timeout the same client identifier is sent again. A duplicate answer means the first attempt
/// landed. Read as a rejection, the platform would close an order the venue holds.
#[test]
fn a_duplicate_link_id_is_a_duplicate_and_the_first_order_is_still_at_the_venue() {
    use crate::execution::classify;

    let outcome = classify(110_072, "OrderLinkedID is duplicate");

    assert_eq!(outcome, VenueOutcome::Rejected(Rejection::Duplicate));
    assert!(!outcome.leaves_no_order(), "a duplicate created no second order, but one exists");
}

/// Same page: 10006 "Too many visits" (the API limit) and 10018 "Exceeded the IP Rate Limit".
#[test]
fn the_documented_rate_limit_codes_are_rate_limits() {
    use crate::execution::classify;

    for code in [10_006, 10_018] {
        assert_eq!(
            classify(code, "Too many visits"),
            VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None }),
            "{code}"
        );
    }
}

/// A code the platform has not read about still travels as itself.
#[test]
fn an_undocumented_code_is_still_unclassified() {
    use crate::execution::classify;

    assert!(matches!(
        classify(199_999, "new"),
        VenueOutcome::Rejected(Rejection::Unclassified { .. })
    ));
}

/// A market topic of an instrument the profile does not list has no scale to be read at.
#[test]
fn a_topic_of_an_unlisted_instrument_is_refused_by_name() {
    let frame = r#"{"topic":"orderbook.50.SOLUSDT","type":"snapshot","ts":1,"data":{"s":"SOLUSDT","b":[],"a":[],"seq":1}}"#;
    assert_eq!(
        parse(frame, &scales()).unwrap_err(),
        ParseError::UnlistedSymbol { topic: "orderbook.50.SOLUSDT".to_string() }
    );
}

/// An account that cannot pay is a classified refusal, not an unclassified code (observed
/// 2026-11-19 on a sub-account holding nothing).
#[test]
fn an_account_that_cannot_pay_is_refused_for_margin() {
    assert_eq!(
        classify(110_007, "ab not enough for new order"),
        VenueOutcome::Rejected(Rejection::InsufficientMargin)
    );
}

/// The venue's `timeInForce` per term. A term it does not take is refused unsent (decision 0009).
#[test]
fn each_term_is_written_as_the_venue_names_it_or_refused() {
    use bijective_model::{OrderTerms, TimeInForce as T};
    let (p, q) = (Scale::new(2).unwrap(), Scale::new(3).unwrap());
    let body = |time_in_force, post_only, price: Option<i128>| {
        let o = OrderRequest {
            terms: OrderTerms {
                time_in_force,
                post_only,
                stop_price_scaled: None,
                expire_at_ns: None,
            },
            price_scaled: price,
            ..order()
        };
        order_body(&o, p, q).map(|b| serde_json::from_str::<serde_json::Value>(&b).unwrap())
    };
    let tif =
        |b: serde_json::Value| b.get("timeInForce").and_then(|v| v.as_str().map(String::from));
    assert_eq!(tif(body(T::GoodTillCancelled, false, Some(100)).unwrap()), None);
    assert_eq!(tif(body(T::ImmediateOrCancel, false, Some(100)).unwrap()).as_deref(), Some("IOC"));
    assert_eq!(tif(body(T::FillOrKill, false, Some(100)).unwrap()).as_deref(), Some("FOK"));
    assert_eq!(
        tif(body(T::GoodTillCancelled, true, Some(100)).unwrap()).as_deref(),
        Some("PostOnly")
    );
    for (term, post, price) in [
        (T::Day, false, Some(100)),
        (T::FillOrKill, false, None),
        (T::ImmediateOrCancel, true, Some(100)),
    ] {
        assert!(matches!(body(term, post, price), Err(RequestError::TermsNotTaken(_))), "{term:?}");
    }
}

/// A stop is a conditional order: the trigger rises for a buy and falls for a sell, on the last
/// price. A stop-market carries no price (decision 0012).
#[test]
fn a_stop_is_sent_as_a_conditional_order() {
    use bijective_model::OrderTerms;
    let (p, q) = (Scale::new(2).unwrap(), Scale::new(3).unwrap());
    let stop = |side, price: Option<i128>| {
        let terms = OrderTerms { stop_price_scaled: Some(250_000), ..OrderTerms::default() };
        let o = OrderRequest { side, terms, price_scaled: price, reduce_only: true, ..order() };
        serde_json::from_str::<serde_json::Value>(&order_body(&o, p, q).unwrap()).unwrap()
    };
    let sell = stop(Side::Sell, None);
    assert_eq!(sell["orderType"], "Market");
    assert_eq!(sell["triggerPrice"], "2500.00");
    assert_eq!(sell["triggerDirection"], 2);
    assert_eq!(sell["triggerBy"], "LastPrice");
    assert_eq!(sell["reduceOnly"], true);
    let buy = stop(Side::Buy, Some(251_000));
    assert_eq!(
        (buy["orderType"].as_str(), buy["triggerDirection"].as_i64()),
        (Some("Limit"), Some(1))
    );
    assert_eq!(buy["price"], "2510.00");
}
