// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue facts this adapter must not smooth over. See `docs/integrations/hyperliquid.md`.

use bijective_venue::contract::{MarketFeed, Sequence};
use bijective_venue::normalise::Scale;

use crate::feed::Feed;
use crate::message::{ParseError, VenueMessage, parse};

fn scales() -> (Scale, Scale) {
    (Scale::new(2).expect("price scale"), Scale::new(4).expect("quantity scale"))
}

fn book_frame(coin: &str, bid: &str, ask: &str) -> Vec<u8> {
    format!(
        r#"{{"channel":"l2Book","data":{{"coin":"{coin}","time":1712345678000,
           "levels":[[{{"px":"{bid}","sz":"1.5","n":3}}],[{{"px":"{ask}","sz":"2.25","n":4}}]]}}}}"#
    )
    .into_bytes()
}

/// A zero sequence would read as the first update ever sent.
#[test]
fn an_observation_from_this_venue_carries_no_sequence() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    let top = feed
        .offer(&book_frame("BTC", "64000.5", "64001.5"), 7)
        .expect("the frame was refused")
        .top
        .expect("both sides were present");

    assert_eq!(top.sequence, Sequence::NotNumbered);
    assert_eq!(top.bid_price, 6_400_050);
    assert_eq!(top.ask_price, 6_400_150);
    assert_eq!(top.venue_ts_ms, 1_712_345_678_000);
}

/// Every book on this venue is whole and unnumbered, so each one is a snapshot that says so.
#[test]
fn a_book_becomes_an_unnumbered_snapshot_record_with_both_instants() {
    use bijective_venue::contract::MarketFact;
    use bijective_venue::contract::market::{BookLevel, ExchangeTime};

    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    let offered = feed.offer(&book_frame("BTC", "64000.5", "64001.5"), 42).expect("understood");

    let record = &offered.records[0];
    assert_eq!(record.received_at_ns, 42);
    assert_eq!(record.exchange_time, ExchangeTime::At { at_ns: 1_712_345_678_000_000_000 });
    let MarketFact::BookSnapshot(book) = &record.fact else { panic!("{record:?}") };
    assert_eq!(book.sequence, Sequence::NotNumbered);
    assert_eq!(book.bids, vec![BookLevel { price_scaled: 6_400_050, quantity_scaled: 15_000 }]);
    assert_eq!(book.asks, vec![BookLevel { price_scaled: 6_400_150, quantity_scaled: 22_500 }]);
}

/// The venue's order is carried, not repaired: a side that is not best first is refused, and the
/// book held before it still stands.
#[test]
fn a_side_that_is_not_best_first_is_refused_and_the_previous_book_stands() {
    use bijective_venue::contract::{FeedError, RecordDefect};

    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    feed.offer(&book_frame("BTC", "64000.5", "64001.5"), 1).expect("understood");

    let rising = br#"{"channel":"l2Book","data":{"coin":"BTC","time":2,"levels":[[{"px":"1","sz":"1"},{"px":"2","sz":"1"}],[]]}}"#;
    assert!(matches!(
        feed.offer(rising, 2),
        Err(FeedError::RecordRefused(RecordDefect::SideOutOfOrder { .. }))
    ));
    assert_eq!(feed.accepted(), 1);
    assert_eq!(feed.instruments(), 1);
}

/// Decimal strings are read as decimals. A float would change the number the venue sent.
#[test]
fn a_figure_the_scale_cannot_hold_is_refused_rather_than_rounded() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    let refused = feed.offer(&book_frame("BTC", "64000.125", "64001.5"), 7);
    assert!(refused.is_err(), "a figure was rounded to fit: {refused:?}");
}

/// One side prices nothing, but it is still what the venue said the book held.
#[test]
fn a_book_with_one_side_produces_a_record_and_no_top() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    let one_sided = br#"{"channel":"l2Book","data":{"coin":"BTC","time":1,"levels":[[{"px":"100","sz":"1"}],[]]}}"#;
    let offered = feed.offer(one_sided, 7).expect("understood");
    assert_eq!(offered.top, None);
    assert_eq!(offered.records.len(), 1);
}

#[test]
fn a_book_that_is_not_two_sided_is_refused() {
    let (price, quantity) = scales();
    let three = br#"{"channel":"l2Book","data":{"coin":"BTC","time":1,"levels":[[],[],[]]}}"#;
    match parse(three, price, quantity) {
        Err(ParseError::Sides { found }) => assert_eq!(found, 3),
        other => panic!("{other:?}"),
    }
}

/// An added channel must not turn every frame into a refusal.
#[test]
fn a_channel_this_build_does_not_consume_is_understood() {
    let (price, quantity) = scales();
    assert_eq!(parse(br#"{"channel":"pong"}"#, price, quantity), Ok(VenueMessage::Pong));
    assert_eq!(
        parse(br#"{"channel":"subscriptionResponse","data":{}}"#, price, quantity),
        Ok(VenueMessage::SubscriptionAccepted)
    );
    assert_eq!(
        parse(br#"{"channel":"candle","data":{}}"#, price, quantity),
        Ok(VenueMessage::Ignored)
    );
}

/// A reconnect cannot be joined to what came before.
#[test]
fn a_reset_forgets_every_book() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    feed.offer(&book_frame("BTC", "64000.5", "64001.5"), 7).expect("understood");
    feed.offer(&book_frame("ETH", "3000.5", "3001.5"), 7).expect("understood");
    assert_eq!(feed.instruments(), 2);
    assert_eq!(feed.accepted(), 2);

    feed.reset();
    assert_eq!(feed.instruments(), 0, "a book survived a reset");
    assert_eq!(feed.accepted(), 2, "the count of what was accepted is not a book");
}

/// The error channel names no cause. Matching on its text would act on an unattributable
/// message.
#[test]
fn the_venues_error_channel_is_reported_and_attributed_to_nothing() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);

    let refused = feed.offer(br#"{"channel":"error","data":"Invalid subscription"}"#, 7);
    assert!(refused.is_err());
    assert!(format!("{refused:?}").contains("Invalid subscription"));
}

use crate::message::Level;
use crate::session::{
    Action, Event, LINK_SILENT_NS, RESUBSCRIBE_LIMIT, STREAM_SILENT_NS, Session, SessionError,
};
use bijective_venue::contract::{ConnectionState, DegradedReason, Moment};

const S: i128 = 1_000_000_000;

fn book_for(symbol: &str) -> VenueMessage {
    VenueMessage::Book {
        symbol: symbol.to_string(),
        bids: vec![Level { price_scaled: 100, quantity_scaled: 1 }],
        asks: vec![Level { price_scaled: 101, quantity_scaled: 1 }],
        venue_ts_ms: 1,
    }
}

fn connected(instruments: &[&str], now: i128) -> Session {
    let mut session =
        Session::new(instruments.iter().map(|s| (*s).to_string()).collect(), 0).expect("session");
    session.step(Event::Tick, 0);
    session.step(Event::Connected, now);
    session
}

#[test]
fn a_session_with_no_instruments_is_refused() {
    match Session::new(Vec::new(), 0) {
        Err(SessionError::NoInstruments) => {}
        Ok(_) => panic!("a session carrying nothing was opened"),
    }
}

/// Proved by a book arriving, not by the acknowledgement.
#[test]
fn an_acknowledgement_does_not_attach_a_subscription() {
    let mut session = connected(&["BTC", "ETH"], 10 * S);

    session.step(Event::Inbound(&VenueMessage::SubscriptionAccepted), 11 * S);
    assert_eq!(session.attached(), 0, "an acknowledgement was read as a subscription");

    session.step(Event::Inbound(&book_for("BTC")), 12 * S);
    assert_eq!(session.attached(), 1);
    assert!(
        matches!(
            session.state(),
            ConnectionState::Degraded {
                reason: DegradedReason::SubscriptionsIncomplete { attached: 1, requested: 2 },
                ..
            }
        ),
        "{:?}",
        session.state()
    );

    session.step(Event::Inbound(&book_for("ETH")), 13 * S);
    assert!(matches!(session.state(), ConnectionState::Connected { .. }), "{:?}", session.state());
}

#[test]
fn a_quiet_stream_is_resubscribed_rather_than_reconnected_around() {
    let mut session = connected(&["BTC", "ETH"], 10 * S);
    session.step(Event::Inbound(&book_for("BTC")), 11 * S);
    session.step(Event::Inbound(&book_for("ETH")), 11 * S);

    let later = 11 * S + STREAM_SILENT_NS + S;
    session.step(Event::Inbound(&book_for("BTC")), later);
    let actions = session.step(Event::Tick, later);

    assert!(
        actions.iter().any(|a| matches!(a, Action::Send(f) if f.contains("ETH"))),
        "the quiet stream was not asked again: {actions:?}"
    );
    assert!(
        !actions.iter().any(|a| matches!(a, Action::Disconnect)),
        "one quiet stream took the socket down with it: {actions:?}"
    );
}

#[test]
fn a_stream_that_never_answers_escalates_to_the_socket() {
    let mut session = connected(&["BTC"], 10 * S);

    let mut now = 10 * S;
    let mut disconnected = false;
    for _ in 0..=RESUBSCRIBE_LIMIT {
        now += STREAM_SILENT_NS + S;
        // The link keeps breathing, so this is not the link idle bound.
        session.step(Event::Inbound(&VenueMessage::Pong), now);
        let actions = session.step(Event::Tick, now);
        if actions.iter().any(|a| matches!(a, Action::Disconnect)) {
            disconnected = true;
            assert!(
                actions.iter().any(|a| matches!(a, Action::ResetBooks)),
                "the socket went down without the books being dropped: {actions:?}"
            );
            break;
        }
    }
    assert!(disconnected, "a stream that never answered was asked forever");
}

#[test]
fn an_idle_link_is_dropped_and_its_books_forgotten() {
    let mut session = connected(&["BTC"], 10 * S);
    session.step(Event::Inbound(&book_for("BTC")), 10 * S);

    let actions = session.step(Event::Tick, 10 * S + LINK_SILENT_NS);
    assert!(actions.iter().any(|a| matches!(a, Action::Disconnect)), "{actions:?}");
    assert!(
        actions.iter().any(|a| matches!(a, Action::ResetBooks)),
        "a book survived a dead link: {actions:?}"
    );
    assert_eq!(session.state(), &ConnectionState::Disconnected);
}

#[test]
fn a_reconnect_resubscribes_everything_and_assumes_nothing() {
    let mut session = connected(&["BTC", "ETH"], 10 * S);
    session.step(Event::Inbound(&book_for("BTC")), 11 * S);
    session.step(Event::Inbound(&book_for("ETH")), 11 * S);
    assert!(matches!(session.state(), ConnectionState::Connected { .. }));

    session.step(Event::SocketClosed, 12 * S);
    let actions = session.step(Event::Connected, 20 * S);

    assert_eq!(session.attached(), 0, "a subscription was carried across a reconnect");
    let sent: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Send(f) => Some(f.clone()),
            _ => None,
        })
        .collect();
    // A book, trades and a context for each of the two instruments.
    assert_eq!(sent.len(), 6, "not every subscription was sent again: {sent:?}");
}

#[test]
fn a_state_change_is_reported_before_the_work_it_causes() {
    let mut session = Session::new(vec!["BTC".to_string()], 0).expect("session");
    let actions = session.step(Event::Tick, 0);
    assert!(
        matches!(actions.first(), Some(Action::Report(ConnectionState::Connecting))),
        "a consumer would have seen the connect before the state that explains it: {actions:?}"
    );
}

#[test]
fn the_instant_on_a_reported_state_is_ours() {
    let session = connected(&["BTC"], 7 * S);
    match session.state() {
        ConnectionState::Degraded { since: Moment::Observed(at), .. } => assert_eq!(*at, 7 * S),
        other => panic!("{other:?}"),
    }
}

/// The waits of a session that fails to connect `failures` times in a row, from one seed.
fn reconnect_waits(seed: u64, failures: u32) -> Vec<i128> {
    let mut s = Session::with_jitter(
        vec!["BTC".to_string()],
        0,
        Box::new(bijective_venue::resilience::SplitMix64::seeded(seed)),
    )
    .unwrap();
    let mut t = 0;
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
        for (index, wait) in reconnect_waits(seed, 8).into_iter().enumerate() {
            let failures = u32::try_from(index).unwrap() + 1;
            let longest = (250_000_000i128 << failures).min(5_000_000_000);
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

/// The `meta` answer as the venue sent it on 2026-11-15, the first entries kept verbatim.
const META_ANSWER: &str = r#"{"universe":[{"szDecimals":5,"name":"BTC","maxLeverage":40,"marginTableId":56},{"szDecimals":4,"name":"ETH","maxLeverage":25,"marginTableId":55},{"szDecimals":1,"name":"MATIC","maxLeverage":20,"marginTableId":20,"isDelisted":true}]}"#;

/// Size decimals give the lot, and the price decimals are six less them (Tick and lot).
#[test]
fn the_venues_size_decimals_give_the_lot_tick_and_price_rule() {
    let observed = crate::observe::read_instrument(META_ANSWER, "ETH").expect("observed");
    let rules = observed.precision.expect("published").rules;

    assert_eq!((rules.quantity_scale, rules.quantity_lot), (4, 1), "0.0001 ETH");
    assert_eq!((rules.price_scale, rules.price_tick), (2, 1), "0.01");
    assert_eq!(
        rules.price_validity,
        bijective_venue::profile::PriceValidity::SignificantFigures { at_most: 5 }
    );
}

/// The coin is what a size counts. The collateral is not in the answer, so it is not observed.
#[test]
fn the_coin_is_observed_and_the_collateral_is_not_assumed() {
    let observed = crate::observe::read_instrument(META_ANSWER, "BTC").expect("observed");
    let coins = observed.currencies.expect("observed");

    assert_eq!(coins.base, "BTC");
    assert_eq!((coins.quote, coins.settlement), (None, None));
}

#[test]
fn an_unlisted_coin_or_an_unreadable_answer_is_refused() {
    assert!(matches!(
        crate::observe::read_instrument(META_ANSWER, "NOPE"),
        Err(crate::observe::ObserveError::UnknownInstrument { .. })
    ));
    assert!(matches!(
        crate::observe::read_instrument("{}", "BTC"),
        Err(crate::observe::ObserveError::Unreadable { .. })
    ));
}
