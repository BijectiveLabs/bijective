// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Trades and the asset context, read from frames the venue sent on 2026-11-06 (public
//! `trades` and `activeAssetCtx` subscriptions for BTC, trimmed to the fields below).

use bijective_venue::contract::MarketFeed;
use bijective_venue::contract::market::{
    Aggressor, ExchangeTime, MarketFact, MarketRecord, OpenInterest, ScaledDecimal,
};
use bijective_venue::normalise::Scale;

use super::Feed;

const TRADES: &str = r#"{"channel":"trades","data":[{"coin":"BTC","side":"B","px":"75421.0","sz":"0.2","time":1793975755775,"hash":"0xba02","tid":1116958894585040,"users":["0x4d68","0xfe1f"]},{"coin":"BTC","side":"A","px":"75420.0","sz":"0.00107","time":1793975755776,"hash":"0xba03","tid":928249555760035,"users":["0x4d68","0x0153"]}]}"#;

const CONTEXT: &str = r#"{"channel":"activeAssetCtx","data":{"coin":"BTC","ctx":{"funding":"0.0000125","openInterest":"42085.67836","prevDayPx":"78861.0","dayNtlVlm":"4703232668.6147642136","premium":"-0.0000967819","oraclePx":"75427.3","markPx":"75420.0","midPx":"75419.5","impactPxs":["75415.1","75420.0"],"dayBaseVlm":"61534.90179"}}}"#;

fn feed() -> Feed {
    Feed::new(Scale::new(2).unwrap(), Scale::new(5).unwrap())
}

fn records(feed: &mut Feed, frame: &str, received_at_ns: i128) -> Vec<MarketRecord> {
    feed.offer(frame.as_bytes(), received_at_ns).expect("a readable frame").records
}

#[test]
fn each_trade_is_a_record_with_the_venues_identity_and_time() {
    let records = records(&mut feed(), TRADES, 5);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].instrument.as_str(), "BTC");
    assert_eq!(records[0].exchange_time, ExchangeTime::from_ms(1_793_975_755_775));
    let MarketFact::Trade(first) = &records[0].fact else { panic!("not a trade") };
    assert_eq!(first.trade_id, "1116958894585040");
    assert_eq!((first.price_scaled, first.quantity_scaled), (7_542_100, 20_000));
    let MarketFact::Trade(second) = &records[1].fact else { panic!("not a trade") };
    assert_eq!(second.trade_id, "928249555760035");
    assert_eq!(second.quantity_scaled, 107);
}

/// The venue's page does not say what `side` means. A minute of live BTC trades set against the
/// book before them put side "A" at the bid and side "B" at the ask far more often than the
/// reverse, which is a taker's side. It is a measurement, not a statement by the venue.
#[test]
fn side_b_is_a_buyer_aggressor_and_side_a_a_seller_aggressor() {
    let records = records(&mut feed(), TRADES, 5);
    let sides: Vec<Aggressor> = records
        .iter()
        .map(|r| match &r.fact {
            MarketFact::Trade(t) => t.aggressor,
            other => panic!("not a trade: {other:?}"),
        })
        .collect();
    assert_eq!(sides, [Aggressor::Buyer, Aggressor::Seller]);
}

#[test]
fn a_side_that_is_neither_letter_is_refused() {
    let odd = TRADES.replace(r#""side":"B""#, r#""side":"X""#);
    assert!(feed().offer(odd.as_bytes(), 5).is_err());
}

/// The context has no time of its own, so the exchange time is not published, and the next funding
/// time is not published either. The rate is the venue's hourly fraction, kept as written.
#[test]
fn the_context_yields_funding_open_interest_mark_and_oracle_as_index() {
    let records = records(&mut feed(), CONTEXT, 9);
    assert!(records.iter().all(|r| r.exchange_time == ExchangeTime::NotPublished));
    let facts: Vec<&MarketFact> = records.iter().map(|r| &r.fact).collect();
    assert!(facts.contains(&&MarketFact::OpenInterest(OpenInterest::BaseQuantity {
        quantity_scaled: 4_208_567_836
    })));
    assert!(facts.contains(&&MarketFact::MarkPrice { mark_price_scaled: 7_542_000 }));
    assert!(facts.contains(&&MarketFact::IndexPrice { index_price_scaled: 7_542_730 }));
    let funding = facts
        .iter()
        .find_map(|f| match f {
            MarketFact::Funding(f) => Some(f),
            _ => None,
        })
        .expect("a funding record");
    assert_eq!(funding.rate, ScaledDecimal { value_scaled: 125, scale: 7 });
    assert_eq!(funding.next_funding_time, ExchangeTime::NotPublished);
}

/// The venue restates the whole context about once a second. Only what moved is a new fact.
#[test]
fn a_restated_context_yields_only_what_changed() {
    let mut feed = feed();
    assert_eq!(records(&mut feed, CONTEXT, 9).len(), 4);
    assert!(records(&mut feed, CONTEXT, 10).is_empty(), "an unchanged context is not a fact");

    let marked = CONTEXT.replace(r#""markPx":"75420.0""#, r#""markPx":"75421.5""#);
    let moved = records(&mut feed, &marked, 11);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].fact, MarketFact::MarkPrice { mark_price_scaled: 7_542_150 });
}

#[test]
fn a_reset_forgets_the_context_so_the_next_one_is_whole_again() {
    let mut feed = feed();
    records(&mut feed, CONTEXT, 9);
    feed.reset();
    assert_eq!(records(&mut feed, CONTEXT, 10).len(), 4);
}
