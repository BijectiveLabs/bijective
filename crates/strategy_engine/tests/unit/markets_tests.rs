// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{Feed, merged_order, sent, stretches};
use bijective_model::market::{ExchangeTime, MarketFact};
use bijective_serialization::signal::facts::FactUpdate;
use bijective_serialization::signal::market::BookUpdate;
use bijective_strategy_engine::Fed;
use bijective_strategy_engine::protocol::{Batch, Sent};

fn top(venue_ts_ms: u64, received_at_ns: Option<i128>) -> Fed {
    Fed::Book(BookUpdate {
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        seq: bijective_model::Sequence::NotNumbered,
        venue_ts_ms,
        received_at_ns,
        bid_price: 1,
        bid_quantity: 1,
        ask_price: 2,
        ask_quantity: 1,
        bid_depth: 1,
        ask_depth: 1,
    })
}

fn mark(received_at_ns: i128) -> Fed {
    Fed::Fact(FactUpdate {
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        exchange_time: ExchangeTime::NotPublished,
        received_at_ns,
        fact: MarketFact::MarkPrice { mark_price_scaled: 3 },
    })
}

fn feed(venue: &str, holds_facts: bool) -> Feed {
    let prefix = if holds_facts { "facts" } else { "market" };
    Feed {
        stream_name: format!("{prefix}-{venue}"),
        venue: Some(venue.to_string()),
        holds_facts,
        opening: None,
    }
}

/// Arrival on this node's clock orders venues, not the venues' own stamps: venue b stamps its top
/// earliest, and it arrived second.
#[test]
fn venues_are_merged_by_arrival_not_by_their_own_stamps() {
    let a = [(0, top(1_000, Some(10))), (1, top(1_001, Some(30)))];
    let b = [(0, top(1, Some(20)))];
    assert_eq!(merged_order(&[&a, &b]), vec![(0, 0), (1, 0), (0, 1)]);
}

/// A top with no receive time is placed by its venue stamp. Each stream keeps its own order even
/// where its stamps run backwards.
#[test]
fn a_top_without_a_receive_time_is_placed_by_its_stamp_and_streams_keep_their_order() {
    let a = [(0, top(5, None)), (1, top(1, None))];
    let b = [(0, top(3, None))];
    assert_eq!(merged_order(&[&a, &b]), vec![(1, 0), (0, 0), (0, 1)]);
}

/// The data engine stores a frame's facts before its top, so at one instant a fact goes first
/// whichever stream was named first.
#[test]
fn a_fact_goes_before_the_top_that_arrived_with_it() {
    let tops = [(0, top(1, Some(20))), (1, top(2, Some(40)))];
    let facts = [(0, mark(20)), (1, mark(30))];
    assert_eq!(merged_order(&[&tops, &facts]), vec![(1, 0), (0, 0), (1, 1), (0, 1)]);
}

#[test]
fn admitted_proposals_are_measured_on_the_market_stream_of_the_venue_they_name() {
    let venue = |v: &str| Some(v.to_string());
    let (east_facts, east, west) = (feed("east", true), feed("east", false), feed("west", false));
    let feeds = [&east_facts, &east, &west];
    let named = [venue("east"), venue("east"), venue("west"), venue("east")];
    assert_eq!(stretches(&named, &feeds), vec![(1, 2), (2, 1), (1, 1)]);
    assert_eq!(stretches(&[None, venue("x")], &[&east_facts, &east]), vec![(1, 2)]);
}

/// A facts stream names no decision position. One market keeps its single batch.
#[test]
fn decisions_are_placed_on_market_streams_only() {
    let (east_facts, east) = (feed("east", true), feed("east", false));
    let one = sent(&[&east_facts, &east], |i| Some(Batch::of(i as u64, i as u64)));
    assert_eq!(one, Sent::One(Batch::of(1, 1)));
}
