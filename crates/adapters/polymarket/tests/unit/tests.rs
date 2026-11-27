// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The parser, the feed and the session on the venue's documented and measured shapes.

use bijective_venue::contract::{FeedError, MarketFeed};
use bijective_venue::normalise::Scale;

use crate::feed::Feed;
use crate::message::{VenueEvent, parse};
use crate::session::{Action, Event, Session};

fn scales() -> (Scale, Scale) {
    (Scale::new(4).expect("price"), Scale::new(6).expect("quantity"))
}

const BOOK: &str = r#"{"event_type":"book","market":"0xm","asset_id":"a1","timestamp":"1000","hash":"h","bids":[{"price":"0.08","size":"10"},{"price":"0.09","size":"5"}],"asks":[{"price":"0.99","size":"7"},{"price":"0.98","size":"3"}]}"#;

fn change(price: &str, size: &str, side: &str, best_bid: &str, best_ask: &str) -> String {
    format!(
        r#"{{"event_type":"price_change","market":"0xm","price_changes":[{{"asset_id":"a1","price":"{price}","size":"{size}","side":"{side}","hash":"h","best_bid":"{best_bid}","best_ask":"{best_ask}"}}],"timestamp":"1001"}}"#
    )
}

/// The venue sends each side from the far end. The platform holds both best first.
#[test]
fn a_book_is_held_best_first_whatever_order_it_arrived_in() {
    let (price, quantity) = scales();
    let events = parse(BOOK.as_bytes(), price, quantity).expect("parsed");
    let [VenueEvent::Book(book)] = events.as_slice() else { panic!("{events:?}") };
    assert_eq!(book.bids[0].price_scaled, 900, "0.09 is the best bid");
    assert_eq!(book.asks[0].price_scaled, 9_800, "0.98 is the best ask");
}

/// A change that leaves the venue's stated best where the held book puts it is a delta. One
/// that does not drops the book, because the held book is then wrong.
#[test]
fn a_change_is_checked_against_the_best_prices_the_venue_states() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    feed.offer(BOOK.as_bytes(), 1).expect("book");

    let raised = change("0.1", "4", "BUY", "0.1", "0.98");
    let offered = feed.offer(raised.as_bytes(), 2).expect("agrees");
    let top = offered.top.expect("both sides");
    assert_eq!((top.bid_price, top.bid_quantity), (1_000, 4_000_000));

    let wrong = change("0.2", "0", "BUY", "0.15", "0.98");
    assert!(matches!(feed.offer(wrong.as_bytes(), 3), Err(FeedError::SequenceBroken { .. })));
    assert_eq!(feed.instruments(), 0, "a book that disagreed with the venue is not kept");
    let next = change("0.1", "1", "BUY", "0.1", "0.98");
    assert!(matches!(feed.offer(next.as_bytes(), 4), Err(FeedError::NoSnapshot { .. })));
}

/// A size of zero removes the level. The best moves to the next one.
#[test]
fn a_zero_size_removes_the_level() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    feed.offer(BOOK.as_bytes(), 1).expect("book");

    let gone = change("0.98", "0", "SELL", "0.09", "0.99");
    let top = feed.offer(gone.as_bytes(), 2).expect("agrees").top.expect("top");
    assert_eq!(top.ask_price, 9_900);
}

/// A figure finer than the platform's scale is refused, never rounded.
#[test]
fn a_price_finer_than_the_scale_is_refused() {
    let (price, quantity) = scales();
    let fine = BOOK.replace("0.08", "0.00805");
    assert!(parse(fine.as_bytes(), price, quantity).is_err());
}

#[test]
fn the_pong_and_an_unknown_event_are_told_apart() {
    let (price, quantity) = scales();
    assert_eq!(parse(b"PONG", price, quantity), Ok(vec![VenueEvent::Pong]));
    assert!(parse(br#"{"event_type":"something_new"}"#, price, quantity).is_err());
    assert_eq!(parse(b"[]", price, quantity), Ok(vec![VenueEvent::Ignored]));
}

/// One opening frame names every outcome. The link is kept with a text `PING`.
#[test]
fn the_session_subscribes_every_outcome_at_once_and_pings_in_text() {
    let mut session = Session::new(vec!["a1".into(), "a2".into()], 0).expect("session");
    assert_eq!(session.step(Event::Tick, 0).last(), Some(&Action::Connect));
    let sent: Vec<String> = session
        .step(Event::Connected, 1)
        .into_iter()
        .filter_map(|a| if let Action::Send(f) = a { Some(f) } else { None })
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].contains(r#""assets_ids":["a1","a2"]"#) && sent[0].contains(r#""type":"market""#)
    );
    assert_eq!(Session::ping_frame(), "PING");
}

/// An outcome with no book for twenty seconds is asked for again on the open link.
#[test]
fn a_quiet_outcome_is_asked_for_again() {
    let mut session = Session::new(vec!["a1".into()], 0).expect("session");
    session.step(Event::Tick, 0);
    session.step(Event::Connected, 1);
    let later = session.step(Event::Tick, 1 + crate::session::STREAM_SILENT_NS);
    assert!(
        later
            .iter()
            .any(|a| matches!(a, Action::Send(f) if f.contains(r#""operation":"subscribe""#))),
        "{later:?}"
    );
}

/// The venue states an empty side as a best price of zero (measured 2026-11-15).
#[test]
fn a_stated_best_of_zero_is_an_empty_side() {
    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    let one_sided = BOOK.replace(
        r#""bids":[{"price":"0.08","size":"10"},{"price":"0.09","size":"5"}]"#,
        r#""bids":[]"#,
    );
    feed.offer(one_sided.as_bytes(), 1).expect("book");

    let more = change("0.99", "9", "SELL", "0", "0.98");
    assert!(feed.offer(more.as_bytes(), 2).is_ok(), "an empty bid side stated as zero agrees");
}

/// A link that is connected stays one state, however many books restate it.
#[test]
fn a_restated_book_does_not_report_the_link_again() {
    let mut session = Session::new(vec!["a1".into()], 0).expect("session");
    session.step(Event::Tick, 0);
    session.step(Event::Connected, 1);
    let (price, quantity) = scales();
    let events = parse(BOOK.as_bytes(), price, quantity).expect("book");
    let first = session.step(Event::Inbound(&events), 2);
    assert!(first.iter().any(|a| matches!(a, Action::Report(_))));
    let again = session.step(Event::Inbound(&events), 3);
    assert!(!again.iter().any(|a| matches!(a, Action::Report(_))), "{again:?}");
}

/// A level a trade consumed can vanish without a change of its own. The stated best says it is
/// gone, so it is removed and the removal recorded (measured 2026-11-15).
#[test]
fn a_level_better_than_the_stated_best_is_removed() {
    use bijective_venue::contract::MarketFact;
    use bijective_venue::contract::market::{BookSide, LevelChange};

    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    feed.offer(BOOK.as_bytes(), 1).expect("book");

    // The 0.98 ask was taken. The venue states the next change with a best ask of 0.99.
    let after = change("0.99", "8", "SELL", "0.09", "0.99");
    let offered = feed.offer(after.as_bytes(), 2).expect("the stated best explains the book");
    assert_eq!(offered.top.expect("top").ask_price, 9_900);
    let MarketFact::BookDelta(delta) = &offered.records[0].fact else { panic!("a delta") };
    assert!(
        delta.changes.contains(&LevelChange::Remove { side: BookSide::Ask, price_scaled: 9_800 })
    );

    // A stated best the held book never had cannot be rebuilt.
    let unknown = change("0.5", "1", "BUY", "0.4", "0.99");
    assert!(matches!(feed.offer(unknown.as_bytes(), 3), Err(FeedError::SequenceBroken { .. })));
}

/// A book the feed lost is asked for once, by leaving and rejoining. A second loss before the
/// book returns asks nothing more.
#[test]
fn a_lost_book_is_asked_for_once_by_rejoining() {
    let mut session = Session::new(vec!["a1".into()], 0).expect("session");
    session.step(Event::Tick, 0);
    session.step(Event::Connected, 1);
    let (price, quantity) = scales();
    let events = parse(BOOK.as_bytes(), price, quantity).expect("book");
    session.step(Event::Inbound(&events), 2);

    let asked = session.step(Event::Resnapshot("a1"), 3);
    let sent: Vec<&String> =
        asked.iter().filter_map(|a| if let Action::Send(f) = a { Some(f) } else { None }).collect();
    assert_eq!(sent.len(), 2, "{asked:?}");
    assert!(sent[0].contains("unsubscribe") && sent[1].contains(r#""operation":"subscribe""#));
    let again = session.step(Event::Resnapshot("a1"), 4);
    assert!(!again.iter().any(|a| matches!(a, Action::Send(_))), "{again:?}");
}

/// The documented `tick_size_change` and `market_resolved` (Real-time data), read whole.
#[test]
fn a_tick_change_and_a_resolution_are_read_with_their_figures() {
    let (price, quantity) = scales();
    let tick = br#"{"event_type":"tick_size_change","market":"0xm","asset_id":"a1","old_tick_size":"0.01","new_tick_size":"0.001","timestamp":"1782753357257"}"#;
    match parse(tick, price, quantity).expect("tick").as_slice() {
        [VenueEvent::TickSize { asset, tick_scaled, ts_ms }] => {
            assert_eq!((asset.as_str(), *tick_scaled, *ts_ms), ("a1", 10, 1_782_753_357_257));
        }
        other => panic!("a tick change: {other:?}"),
    }

    let resolved = br#"{"event_type":"market_resolved","id":"1","market":"0xm","assets_ids":["a1","a2"],"winning_asset_id":"a1","winning_outcome":"Yes","timestamp":"1782753357257"}"#;
    match parse(resolved, price, quantity).expect("resolved").as_slice() {
        [VenueEvent::Resolved { assets, winning, .. }] => {
            assert_eq!(assets, &vec!["a1".to_string(), "a2".to_string()]);
            assert_eq!(winning, "a1");
        }
        other => panic!("a resolution: {other:?}"),
    }
}

/// The documented `market_resolved` event (Real-time data): each held outcome is recorded won or
/// lost, an outcome never sent a book is not, and a winner outside the outcomes is refused.
#[test]
fn a_resolution_records_each_held_outcome_as_won_or_lost() {
    use bijective_model::market::MarketFact;

    let (price, quantity) = scales();
    let mut feed = Feed::new(price, quantity);
    for asset in ["a1", "a2"] {
        let book = format!(
            r#"{{"event_type":"book","asset_id":"{asset}","market":"m","bids":[{{"price":"0.4","size":"10"}}],"asks":[{{"price":"0.6","size":"10"}}],"timestamp":"1700000000000"}}"#
        );
        feed.offer(book.as_bytes(), 1).expect("book");
    }
    let resolved = r#"{"event_type":"market_resolved","id":"1","market":"m","assets_ids":["a1","a2","a3"],"winning_asset_id":"a2","winning_outcome":"No","timestamp":"1700000001000"}"#;

    let offered = feed.offer(resolved.as_bytes(), 2).expect("resolved");

    let read: Vec<(String, MarketFact)> =
        offered.records.iter().map(|r| (r.instrument.to_string(), r.fact.clone())).collect();
    assert_eq!(
        read,
        [
            ("a1".to_string(), MarketFact::Resolution { won: false }),
            ("a2".to_string(), MarketFact::Resolution { won: true }),
        ]
    );
    let stray = resolved.replace(r#""winning_asset_id":"a2""#, r#""winning_asset_id":"z9""#);
    assert!(feed.offer(stray.as_bytes(), 3).is_err());
}
