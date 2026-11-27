// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the protocol must refuse, and the one property everything else rests on.

use super::*;

fn proposal_of(read: &Decided) -> &Proposal {
    match read {
        Decided::Proposal { proposal, .. } => proposal.as_ref(),
        Decided::Turn => panic!("a proposal was expected and a turn arrived"),
    }
}

fn at_of(read: &Decided) -> DecidedAt {
    match read {
        Decided::Proposal { at, .. } => *at,
        Decided::Turn => panic!("a proposal was expected and a turn arrived"),
    }
}

fn update() -> BookUpdate {
    BookUpdate {
        instrument: InstrumentId::new("btc-perp"),
        seq: Sequence::Numbered(4_102),
        venue_ts_ms: 1_788_000_000_000,
        received_at_ns: None,
        bid_price: 2_654_375,
        bid_quantity: 1_500,
        ask_price: 2_654_500,
        ask_quantity: 900,
        bid_depth: 5,
        ask_depth: 5,
    }
}

fn proposal_line(price: Option<&str>, cap: Option<&str>) -> String {
    let mut fields = vec![
        r#""emit":"proposal""#.to_string(),
        r#""desk":"alpha""#.to_string(),
        r#""instrument":"btc-perp""#.to_string(),
        r#""side":"buy""#.to_string(),
        r#""quantity_scaled":"1500""#.to_string(),
        r#""client_id":"abc123""#.to_string(),
        r#""decided_at_ns":"1788000000000000000""#.to_string(),
    ];
    if let Some(p) = price {
        fields.push(format!(r#""price_scaled":"{p}""#));
    }
    if let Some(c) = cap {
        fields.push(format!(r#""cap_scaled":"{c}""#));
    }
    format!("{{{}}}", fields.join(","))
}

/// The figure exceeds a double's exact range. As a JSON number it would silently change.
#[test]
fn a_wide_figure_crosses_without_losing_a_digit() {
    let exact: i128 = 9_007_199_254_740_993; // 2^53 + 1
    let line = proposal_line(Some(&exact.to_string()), None);

    let read = read_proposal(&line, &crate::protocol::Sent::from(Batch::of(0, u64::MAX)), true)
        .expect("a whole number in text");
    assert_eq!(proposal_of(&read).price_scaled, Some(exact));

    // The same figure through a JSON *number* does not survive, which is why it is text.
    let as_double: f64 = exact as f64;
    assert_ne!(as_double as i128, exact, "the premise of this test no longer holds");
}

#[test]
fn a_book_update_renders_every_figure_as_text() {
    let line = to_line(&book_line(7, &update(), None)).expect("serialise");

    assert!(line.ends_with('\n'), "a message is a whole line");
    assert!(line.contains(r#""bid_price":"2654375""#), "{line}");
    assert!(line.contains(r#""venue_ts_ms":"1788000000000""#), "{line}");
    // Depth is a count, not a scaled amount, so it stays a number.
    assert!(line.contains(r#""bid_depth":5"#), "{line}");
}

#[test]
fn a_venue_that_does_not_number_sends_no_number() {
    let mut u = update();
    u.seq = Sequence::NotNumbered;

    let line = to_line(&book_line(7, &u, None)).expect("serialise");
    assert!(!line.contains("venue_sequence"), "an absent number was sent as one: {line}");

    let numbered = to_line(&book_line(7, &update(), None)).expect("serialise");
    assert!(numbered.contains(r#""venue_sequence":"4102""#), "{numbered}");
}

#[test]
fn the_end_of_a_window_is_a_line_like_any_other() {
    let line = to_line(&Inbound::EndOfWindow).expect("serialise");
    assert_eq!(line, "{\"event\":\"end_of_window\"}\n");
}

#[test]
fn a_limit_order_is_read_with_its_price() {
    let read = read_proposal(
        &proposal_line(Some("2654375"), None),
        &crate::protocol::Sent::from(Batch::of(0, u64::MAX)),
        true,
    )
    .expect("a limit order");

    assert_eq!(proposal_of(&read).price_scaled, Some(2_654_375));
    assert_eq!(proposal_of(&read).quantity_scaled, 1_500);
    assert_eq!(proposal_of(&read).desk, "alpha");
    assert_eq!(proposal_of(&read).client_id, "abc123");
}

#[test]
fn a_market_order_with_nothing_its_author_is_held_to_is_refused() {
    assert_eq!(
        read_proposal(
            &proposal_line(None, None),
            &crate::protocol::Sent::from(Batch::of(0, u64::MAX)),
            true
        )
        .expect_err("unbounded cost"),
        LineError::MarketOrderWithoutCap
    );
}

#[test]
fn a_market_order_with_a_cap_is_read() {
    let read = read_proposal(
        &proposal_line(None, Some("9000000")),
        &crate::protocol::Sent::from(Batch::of(0, u64::MAX)),
        true,
    )
    .expect("a capped market order");
    assert_eq!(proposal_of(&read).price_scaled, None);
    assert_eq!(proposal_of(&read).cap_scaled, Some(9_000_000));
}

/// A figure sent as a JSON number does not parse. A strategy cannot opt out of text.
#[test]
fn a_figure_sent_as_a_number_rather_than_text_is_refused() {
    let line = r#"{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":1500,"price_scaled":"1","client_id":"a","decided_at_ns":"1"}"#;

    assert!(
        matches!(
            read_proposal(line, &crate::protocol::Sent::from(Batch::of(0, u64::MAX)), true)
                .expect_err("a bare number"),
            LineError::Unreadable { .. }
        ),
        "a scaled amount was accepted as a JSON number"
    );
}

#[test]
fn a_line_that_is_not_a_proposal_is_refused_rather_than_part_read() {
    for line in [r#"{"emit":"something_else"}"#, r#"{"desk":"alpha"}"#, "not json at all", ""] {
        assert!(
            matches!(
                read_proposal(line, &crate::protocol::Sent::from(Batch::of(0, u64::MAX)), true),
                Err(LineError::Unreadable { .. })
            ),
            "accepted: {line}"
        );
    }
}

#[test]
fn a_quantity_that_is_not_a_whole_number_is_refused_and_named() {
    let line = proposal_line(Some("1"), None)
        .replace(r#""quantity_scaled":"1500""#, r#""quantity_scaled":"1.5""#);

    assert_eq!(
        read_proposal(&line, &crate::protocol::Sent::from(Batch::of(0, u64::MAX)), true)
            .expect_err("a fraction"),
        LineError::NotAWholeNumber { field: "quantity_scaled", found: "1.5".into() }
    );
}

#[test]
fn a_line_is_read_with_or_without_its_newline() {
    let line = proposal_line(Some("2654375"), None);
    assert_eq!(
        read_proposal(&line, &crate::protocol::Sent::from(Batch::of(0, u64::MAX)), true)
            .expect("bare"),
        read_proposal(
            &format!("{line}\n"),
            &crate::protocol::Sent::from(Batch::of(0, u64::MAX)),
            true
        )
        .expect("with newline")
    );
}

#[test]
fn a_resumed_session_states_the_gap_it_is_resuming_after() {
    let line = to_line(&Inbound::Resumed {
        from_position: "9007199254740993".to_string(),
        missed: Missed::From { position: "42".to_string() },
        venue: Some("venue-a".to_string()),
        stream: Some("market-venue-a".to_string()),
    })
    .expect("render");

    assert!(line.contains(r#""event":"resumed""#), "{line}");
    assert!(
        line.contains(r#""from_position":"9007199254740993""#),
        "the position must survive as text: {line}"
    );
    assert!(line.contains(r#""missed":"from""#), "{line}");
    assert!(line.contains(r#""position":"42""#), "{line}");
    assert!(line.contains(r#""venue":"venue-a""#), "{line}");
}

#[test]
fn a_gap_nothing_records_is_said_rather_than_left_out() {
    let line = to_line(&Inbound::Resumed {
        from_position: "7".to_string(),
        missed: Missed::NotRecorded,
        venue: None,
        stream: None,
    })
    .expect("render");

    assert!(line.contains(r#""missed":"not_recorded""#), "{line}");
}

#[test]
fn an_event_this_build_does_not_know_is_not_an_error_for_the_strategy() {
    let read = read_event(r#"{"event":"something_added_after_this_build","field":1}"#);

    assert_eq!(read, Ok(Inbound::Unrecognised));
}

/// The tolerance is one-directional: what an operator emits is still refused rather than guessed.
#[test]
fn a_proposal_this_build_does_not_know_is_still_refused() {
    let read = read_proposal(
        r#"{"emit":"something_added_after_this_build"}"#,
        &crate::protocol::Sent::from(Batch::of(0, u64::MAX)),
        true,
    );

    assert!(
        matches!(read, Err(LineError::Unreadable { .. })),
        "an unknown emission must be refused, not passed over: {read:?}"
    );
}

#[test]
fn an_event_this_build_does_know_is_read_as_itself() {
    let line = to_line(&book_line(7, &update(), None)).expect("render");

    match read_event(&line) {
        Ok(Inbound::Book { instrument, .. }) => assert_eq!(instrument, "btc-perp"),
        other => panic!("a book line must read as one: {other:?}"),
    }
}

#[test]
fn a_book_line_carries_the_position_the_event_sits_at() {
    let line = to_line(&book_line(4_102, &update(), None)).expect("render");

    assert!(line.contains(r#""position":"4102""#), "{line}");
}

#[test]
fn a_decision_quoted_against_an_event_that_was_sent_is_read_with_it() {
    let line = r#"{"emit":"proposal","desk":"a","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c","decided_at_ns":"1","decided_at_position":"3"}"#;

    let read = read_proposal(line, &crate::protocol::Sent::from(Batch::of(0, 4)), true)
        .expect("within what was sent");

    assert_eq!(at_of(&read), DecidedAt::Position(3));
}

#[test]
fn a_decision_quoted_past_what_was_sent_is_refused() {
    let line = r#"{"emit":"proposal","desk":"a","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c","decided_at_ns":"1","decided_at_position":"9"}"#;

    match read_proposal(line, &crate::protocol::Sent::from(Batch::of(0, 4)), true) {
        Err(LineError::DecidedOutsideTheBatch { at, batch }) => {
            assert_eq!(at, 9);
            assert_eq!(batch.through, 4);
        }
        other => panic!("a decision on what was never sent must be refused: {other:?}"),
    }
}

/// A strategy that says nothing about its own event says nothing, and is not read as position zero.
#[test]
fn a_decision_that_names_no_event_is_not_one_made_at_the_first() {
    let read = read_proposal(
        &proposal_line(Some("2654375"), None),
        &crate::protocol::Sent::from(Batch::of(0, 4)),
        true,
    )
    .expect("a limit order");

    assert_eq!(at_of(&read), DecidedAt::NotSaid);
}

/// Before the batch is refused too: pricing there is look-ahead pointed the other way.
#[test]
fn a_decision_claimed_on_an_event_already_answered_is_refused() {
    let line = r#"{"emit":"proposal","desk":"a","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c","decided_at_ns":"1","decided_at_position":"2"}"#;

    match read_proposal(line, &crate::protocol::Sent::from(Batch::of(5, 9)), true) {
        Err(LineError::DecidedOutsideTheBatch { at, batch }) => {
            assert_eq!(at, 2);
            assert_eq!((batch.from, batch.through), (5, 9));
        }
        other => panic!("a decision before the batch must be refused: {other:?}"),
    }
}

#[test]
fn a_decision_anywhere_inside_the_batch_is_read() {
    for at in ["5", "7", "9"] {
        let line = format!(
            r#"{{"emit":"proposal","desk":"a","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c","decided_at_ns":"1","decided_at_position":"{at}"}}"#
        );

        assert!(
            read_proposal(&line, &crate::protocol::Sent::from(Batch::of(5, 9)), true).is_ok(),
            "{at} is inside [5, 9]"
        );
    }
}

#[test]
fn a_decision_position_from_a_run_that_cannot_place_one_is_refused() {
    let line = r#"{"emit":"proposal","desk":"a","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c","decided_at_ns":"1","decided_at_position":"2"}"#;

    assert!(matches!(
        read_proposal(line, &crate::protocol::Sent::from(Batch::of(0, 4)), false),
        Err(LineError::DecisionUnplaceable)
    ));
}

/// Saying nothing is still fine there: what is refused is the claim, not the absence of output.
#[test]
fn saying_nothing_is_read_by_a_run_that_cannot_place_a_decision() {
    let read = read_proposal(
        &proposal_line(Some("2654375"), None),
        &crate::protocol::Sent::from(Batch::of(0, 4)),
        false,
    )
    .expect("a limit order");

    assert_eq!(at_of(&read), DecidedAt::NotSaid);
}

#[test]
fn the_turn_marker_reads_as_a_turn() {
    assert!(is_turn(r#"{"emit":"answered"}"#));
    assert!(!is_turn(&proposal_line(Some("1"), None)));
    assert_eq!(
        read_proposal(
            r#"{"emit":"answered"}"#,
            &crate::protocol::Sent::from(Batch::of(0, 1)),
            true
        ),
        Ok(Decided::Turn)
    );
}

/// A top says where it was quoted, and a proposal may say where it is for. Neither is required.
#[test]
fn the_venue_travels_in_both_directions_and_is_absent_when_unsaid() {
    let named = to_line(&book_line(7, &update(), Some("venue-a"))).expect("serialise");
    assert!(named.contains(r#""venue":"venue-a""#), "{named}");
    let unnamed = to_line(&book_line(7, &update(), None)).expect("serialise");
    assert!(!unnamed.contains("venue\""), "{unnamed}");

    let line = proposal_line(Some("2654375"), None).replace("}", r#","venue":"venue-b"}"#);
    let Decided::Proposal { proposal, .. } =
        read_proposal(&line, &crate::protocol::Sent::from(Batch::of(0, u64::MAX)), true)
            .expect("a proposal naming a venue")
    else {
        panic!("not a proposal");
    };
    assert_eq!(proposal.venue.as_deref(), Some("venue-b"));
    let Decided::Proposal { proposal, .. } = read_proposal(
        &proposal_line(Some("2654375"), None),
        &crate::protocol::Sent::from(Batch::of(0, u64::MAX)),
        true,
    )
    .expect("a proposal") else {
        panic!("not a proposal");
    };
    assert_eq!(proposal.venue, None, "unsaid is left to the desk, not defaulted");
}

/// A fill is told with its figures as text. A refusal names no order and carries why.
#[test]
fn what_became_of_a_proposal_is_told_with_its_figures_as_text() {
    use bijective_serialization::signal::feedback::{Feedback, Outcome};
    let filled = Feedback {
        proposal_position: 12,
        client_id: Some("c-1".into()),
        outcome: Outcome::Filled {
            fill_id: "f-9".into(),
            quantity_scaled: 1_500,
            price_scaled: 2_654_375,
        },
        reason: String::new(),
        control_position: 40,
        at_ns: 1,
    };
    let line = to_line(&order_line(&filled)).expect("render");
    assert!(line.contains(r#""event":"order""#), "{line}");
    assert!(line.contains(r#""proposal_position":"12""#), "{line}");
    assert!(
        line.contains(r#""status":"filled""#) && line.contains(r#""fill_price_scaled":"2654375""#),
        "{line}"
    );
    assert!(!line.contains("reason"), "an empty reason is left out: {line}");
    assert!(line.contains(r#""recorded_at_ns":"1""#), "{line}");

    let refused = Feedback {
        client_id: None,
        outcome: Outcome::Refused,
        reason: "Unreadable".into(),
        ..filled
    };
    let line = to_line(&order_line(&refused)).expect("render");
    assert!(
        line.contains(r#""status":"refused""#) && line.contains(r#""reason":"Unreadable""#),
        "{line}"
    );
    assert!(!line.contains("client_id") && !line.contains("fill_"), "{line}");
}

/// Reading several venues, a position is a position on a venue: the decision names it, and is
/// checked against what that venue was sent.
#[test]
fn a_proposal_across_venues_names_a_venue_it_was_sent() {
    let sent = Sent::PerVenue(
        [("venue-a".to_string(), Batch::of(4, 4)), ("venue-b".to_string(), Batch::of(9, 9))]
            .into_iter()
            .collect(),
    );
    let with = |venue: &str, at: &str| {
        proposal_line(Some("2654375"), None)
            .replace("}", &format!(r#","venue":"{venue}","decided_at_position":"{at}"}}"#))
    };
    assert!(read_proposal(&with("venue-b", "9"), &sent, true).is_ok());
    assert_eq!(
        read_proposal(&with("venue-b", "4"), &sent, true),
        Err(LineError::DecidedOutsideTheBatch { at: 4, batch: Batch::of(9, 9) })
    );
    assert_eq!(
        read_proposal(&with("venue-c", "4"), &sent, true),
        Err(LineError::VenueNotSent { venue: Some("venue-c".to_string()) })
    );
    let unnamed =
        proposal_line(Some("2654375"), None).replace("}", r#","decided_at_position":"4"}"#);
    assert_eq!(read_proposal(&unnamed, &sent, true), Err(LineError::VenueNotSent { venue: None }));
}

/// A fact is one event named by its kind, every figure as text, and reads back as it was sent.
#[test]
fn a_fact_line_names_its_kind_and_keeps_every_figure_as_text() {
    use bijective_model::market::{Aggressor, ExchangeTime, MarketFact, Trade};
    use bijective_serialization::signal::facts::FactUpdate;

    let update = FactUpdate {
        instrument: InstrumentId::new("btc-perp"),
        exchange_time: ExchangeTime::At { at_ns: 1_788_000_000_000_000_000 },
        received_at_ns: 1_788_000_000_001_000_000,
        fact: MarketFact::Trade(Trade {
            trade_id: "t-1".into(),
            aggressor: Aggressor::Seller,
            price_scaled: 2_654_375,
            quantity_scaled: 1_500,
        }),
    };
    let event = fact_line(4, &update, Some("venue-a")).expect("a trade is a fact");
    let line = to_line(&event).expect("render");
    for part in [
        r#""event":"fact""#,
        r#""kind":"trade""#,
        r#""fact_position":"4""#,
        r#""venue":"venue-a""#,
        r#""aggressor":"seller""#,
        r#""price_scaled":"2654375""#,
        r#""received_at_ns":"1788000000001000000""#,
    ] {
        assert!(line.contains(part), "{part} missing from {line}");
    }
    assert_eq!(read_event(&line).expect("read back"), event);
}

/// A proposal states its terms by name. An unknown name is refused, never read as the default.
#[test]
fn a_proposal_states_its_terms_and_an_unknown_time_in_force_is_refused() {
    let sent = crate::protocol::Sent::from(Batch::of(0, u64::MAX));
    let line = proposal_line(Some("100"), None);
    let with = |extra: &str| format!("{},{extra}}}", line.trim_end_matches('}'));
    let read = read_proposal(&with(r#""time_in_force":"ioc""#), &sent, true).expect("ioc");
    assert_eq!(
        proposal_of(&read).terms.time_in_force,
        bijective_model::TimeInForce::ImmediateOrCancel
    );
    let read = read_proposal(&with(r#""post_only":true"#), &sent, true).expect("post-only");
    assert!(proposal_of(&read).terms.post_only);
    let stop = with(r#""stop_price_scaled":"250000","reduce_only":true"#);
    let read = read_proposal(&stop, &sent, true).expect("a protective stop");
    assert_eq!(proposal_of(&read).terms.stop_price_scaled, Some(250_000));
    let dated = read_proposal(&with(r#""expire_at_ns":"1800000000000000000""#), &sent, true);
    assert_eq!(
        proposal_of(&dated.expect("dated")).terms.expire_at_ns,
        Some(1_800_000_000_000_000_000)
    );
    assert!(proposal_of(&read).reduce_only);
    assert!(
        !proposal_of(&read_proposal(&with(r#""post_only":false"#), &sent, true).unwrap())
            .reduce_only
    );
    assert!(matches!(
        read_proposal(&with(r#""time_in_force":"gtd""#), &sent, true),
        Err(crate::protocol::LineError::UnknownTimeInForce { .. })
    ));
}

/// A child names its parent and group. A proposal that names neither is not linked.
#[test]
fn a_proposal_names_its_parent_and_group() {
    let sent = crate::protocol::Sent::from(Batch::of(0, u64::MAX));
    let line = proposal_line(Some("100"), None);
    let child = format!(
        "{},{}}}",
        line.trim_end_matches('}'),
        r#""reduce_only":true,"parent":"entry-1","oco":"bracket""#
    );
    let read = read_proposal(&child, &sent, true).expect("a child");
    assert_eq!(proposal_of(&read).parent.as_deref(), Some("entry-1"));
    assert_eq!(proposal_of(&read).oco.as_deref(), Some("bracket"));
    let plain = read_proposal(&line, &sent, true).expect("a plain proposal");
    assert!(proposal_of(&plain).parent.is_none() && proposal_of(&plain).oco.is_none());
}
