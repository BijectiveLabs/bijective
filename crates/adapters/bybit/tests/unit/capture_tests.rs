// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Trades, liquidations and the derivatives ticker, read from the frames the venue documents.
//! The frames are the documentation's own examples (Bybit V5 public WebSocket: Trade, Ticker,
//! All Liquidation), read 2026-11-06.

use bijective_model::Side;
use bijective_model::market::{
    Aggressor, ExchangeTime, MarketFact, MarketRecord, OpenInterest, ScaledDecimal,
};
use bijective_venue::contract::MarketFeed;

use super::Feed;

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

const TRADE: &str = r#"{"topic":"publicTrade.BTCUSDT","type":"snapshot","ts":1672304486868,"data":[{"T":1672304486865,"s":"BTCUSDT","S":"Buy","v":"0.001","p":"16578.50","L":"PlusTick","i":"20f43950-d8dd-5b31-9112-a178eb6023af","BT":false,"seq":1783284617}]}"#;

const LIQUIDATION: &str = r#"{"topic":"allLiquidation.ROSEUSDT","type":"snapshot","ts":1739502303204,"data":[{"T":1739502302929,"s":"ROSEUSDT","S":"Sell","v":"20000","p":"0.04499"}]}"#;

const TICKER_SNAPSHOT: &str = r#"{"topic":"tickers.BTCUSDT","type":"snapshot","data":{"symbol":"BTCUSDT","fundingRate":"0.0001","nextFundingTime":"1760342400000","openInterest":"492373.72","markPrice":"66666.60","indexPrice":"115418.19"},"ts":1760325052630}"#;

/// Frames the venue sent on 2026-11-06 (public linear stream, BTCUSDT), with the fields this
/// adapter does not read left in, so the parse is shown to ignore them.
const REAL_TICKER_SNAPSHOT: &str = r#"{"topic":"tickers.BTCUSDT","type":"snapshot","data":{"symbol":"BTCUSDT","tickDirection":"PlusTick","price24hPcnt":"-0.020963","lastPrice":"75615.10","markPrice":"75639.51","indexPrice":"75667.52","openInterest":"70498.495","openInterestValue":"5332471617.54","fundingIntervalHour":"8","fundingCap":"0.00333","nextFundingTime":"1793980800000","fundingRate":"0.00005671","bid1Price":"75615.10","bid1Size":"9.224","preOpenPrice":"","preQty":"","curPreListingPhase":""},"cs":840560286082,"ts":1793978431421}"#;

const REAL_TICKER_DELTA: &str = r#"{"topic":"tickers.BTCUSDT","type":"delta","data":{"symbol":"BTCUSDT","tickDirection":"MinusTick","lastPrice":"75610.00","bid1Price":"75609.90","bid1Size":"2.639"},"cs":840560290562,"ts":1793978431521}"#;

fn feed() -> Feed {
    Feed::new(at(2, 3))
}

fn records(feed: &mut Feed, frame: &str, received_at_ns: i128) -> Vec<MarketRecord> {
    feed.offer(frame.as_bytes(), received_at_ns).expect("a readable frame").records
}

#[test]
fn a_trade_is_one_record_with_the_venues_identity_and_the_takers_side() {
    let records = records(&mut feed(), TRADE, 5);
    assert_eq!(records.len(), 1);
    let MarketRecord { instrument, exchange_time, received_at_ns, fact } = &records[0];
    assert_eq!(instrument.as_str(), "BTCUSDT");
    assert_eq!(*exchange_time, ExchangeTime::from_ms(1_672_304_486_865));
    assert_eq!(*received_at_ns, 5);
    let MarketFact::Trade(trade) = fact else { panic!("not a trade: {fact:?}") };
    assert_eq!(trade.trade_id, "20f43950-d8dd-5b31-9112-a178eb6023af");
    assert_eq!(trade.aggressor, Aggressor::Buyer);
    assert_eq!((trade.price_scaled, trade.quantity_scaled), (1_657_850, 1));
}

#[test]
fn a_sell_taker_is_a_seller_aggressor() {
    let sell = TRADE.replace(r#""S":"Buy""#, r#""S":"Sell""#);
    let records = records(&mut feed(), &sell, 5);
    let MarketFact::Trade(trade) = &records[0].fact else { panic!("not a trade") };
    assert_eq!(trade.aggressor, Aggressor::Seller);
}

/// The venue states the side of the liquidated POSITION. The record states the side of the forced
/// ORDER, which is the opposite. A long (Buy) closed by force is closed by a sell.
#[test]
fn a_liquidation_records_the_forced_order_not_the_position() {
    let mut feed = Feed::new(at(5, 0));
    let short_liquidated = records(&mut feed, LIQUIDATION, 7);
    let MarketFact::Liquidation(l) = &short_liquidated[0].fact else { panic!("not a liquidation") };
    assert_eq!(l.forced_order_side, Side::Buy, "a short position is closed by a forced buy");
    assert_eq!((l.price_scaled, l.quantity_scaled), (4_499, 20_000));

    let long = LIQUIDATION.replace(r#""S":"Sell""#, r#""S":"Buy""#);
    let long_liquidated = records(&mut feed, &long, 8);
    let MarketFact::Liquidation(l) = &long_liquidated[0].fact else { panic!("not a liquidation") };
    assert_eq!(l.forced_order_side, Side::Sell);
}

#[test]
fn a_ticker_snapshot_yields_funding_open_interest_mark_and_index() {
    let records = records(&mut feed(), TICKER_SNAPSHOT, 9);
    let facts: Vec<&MarketFact> = records.iter().map(|r| &r.fact).collect();
    assert!(records.iter().all(|r| r.exchange_time == ExchangeTime::from_ms(1_760_325_052_630)));
    assert!(facts.contains(&&MarketFact::OpenInterest(OpenInterest::BaseQuantity {
        quantity_scaled: 492_373_720
    })));
    assert!(facts.contains(&&MarketFact::MarkPrice { mark_price_scaled: 6_666_660 }));
    assert!(facts.contains(&&MarketFact::IndexPrice { index_price_scaled: 11_541_819 }));
    let funding = facts
        .iter()
        .find_map(|f| match f {
            MarketFact::Funding(f) => Some(f),
            _ => None,
        })
        .expect("a funding record");
    // The rate is kept exactly as the venue wrote it: "0.0001" is 1 at scale 4. No unit is assumed.
    assert_eq!(funding.rate, ScaledDecimal { value_scaled: 1, scale: 4 });
    assert_eq!(funding.next_funding_time, ExchangeTime::from_ms(1_760_342_400_000));
}

/// A delta carries only what changed. A mark price alone is a mark record and nothing else. A
/// funding rate alone is funding only because the next funding time was already known.
/// The live ticker carries many fields this adapter does not use. A delta without any it uses
/// makes no record. The funding rate is a fraction of the position per funding interval: the same
/// frame states a cap of 0.00333 and an interval of 8 hours, and this one reads 0.00005671.
#[test]
fn the_live_ticker_yields_its_records_and_a_delta_without_them_yields_none() {
    let mut feed = feed();
    let snapshot = records(&mut feed, REAL_TICKER_SNAPSHOT, 1);
    let funding = snapshot
        .iter()
        .find_map(|r| match &r.fact {
            MarketFact::Funding(f) => Some(f),
            _ => None,
        })
        .expect("a funding record");
    assert_eq!(funding.rate, ScaledDecimal { value_scaled: 5_671, scale: 8 });
    assert_eq!(snapshot.len(), 4);
    assert!(records(&mut feed, REAL_TICKER_DELTA, 2).is_empty());
}

#[test]
fn a_ticker_delta_yields_only_what_it_carries() {
    let mut feed = feed();
    records(&mut feed, TICKER_SNAPSHOT, 9);

    let mark_only = r#"{"topic":"tickers.BTCUSDT","type":"delta","data":{"symbol":"BTCUSDT","markPrice":"66670.10"},"ts":1760325052730}"#;
    let delta = records(&mut feed, mark_only, 10);
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0].fact, MarketFact::MarkPrice { mark_price_scaled: 6_667_010 });

    let rate_only = r#"{"topic":"tickers.BTCUSDT","type":"delta","data":{"symbol":"BTCUSDT","fundingRate":"-0.00005"},"ts":1760325052830}"#;
    let funding = records(&mut feed, rate_only, 11);
    let MarketFact::Funding(f) = &funding[0].fact else { panic!("not funding") };
    assert_eq!(f.rate, ScaledDecimal { value_scaled: -5, scale: 5 });
    assert_eq!(
        f.next_funding_time,
        ExchangeTime::from_ms(1_760_342_400_000),
        "carried from before"
    );
}

/// Without the next funding time there is no funding record to make: a rate alone would state a
/// fact the record contract cannot hold, and inventing the time would state one the venue did not.
#[test]
fn a_funding_rate_with_no_known_next_time_makes_no_funding_record() {
    let rate_first = r#"{"topic":"tickers.BTCUSDT","type":"delta","data":{"symbol":"BTCUSDT","fundingRate":"0.0001"},"ts":1760325052830}"#;
    assert!(records(&mut feed(), rate_first, 1).is_empty());
}

#[test]
fn a_trade_the_contract_refuses_is_refused_not_dropped() {
    let zero_price = TRADE.replace(r#""p":"16578.50""#, r#""p":"0.00""#);
    assert!(feed().offer(zero_price.as_bytes(), 5).is_err());
    let unreadable = TRADE.replace(r#""p":"16578.50""#, r#""p":"abc""#);
    assert!(feed().offer(unreadable.as_bytes(), 5).is_err());
}

#[test]
fn a_derivatives_connection_carries_four_topics_per_instrument_and_spot_two() {
    use crate::stream::topics_for;
    let symbols = ["BTCUSDT".to_string()];
    assert_eq!(
        topics_for("wss://stream.bybit.com/v5/public/linear", &symbols),
        [
            "orderbook.50.BTCUSDT",
            "publicTrade.BTCUSDT",
            "tickers.BTCUSDT",
            "allLiquidation.BTCUSDT"
        ]
    );
    assert_eq!(
        topics_for("wss://stream.bybit.com/v5/public/spot", &symbols),
        ["orderbook.50.BTCUSDT", "publicTrade.BTCUSDT"]
    );
}
