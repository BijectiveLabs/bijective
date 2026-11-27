// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The record contract, each rule shown to refuse the record that breaks it, and the feed
//! conformance checks proven against feeds built to fail them.

use bijective_model::market::{
    Aggressor, BookDelta, BookLevel, BookSide, BookSnapshot, ExchangeTime, Funding, LevelChange,
    Liquidation, LongShortRatio, MarketFact, MarketRecord, OpenInterest, ScaledDecimal, Trade,
};
use bijective_model::{Sequence, Side, VenueSymbol};

use super::{RecordDefect, check_record, class_of};
use crate::contract::conformance::{CheckOutcome, CheckResult, RecordedFrame, check_feed};
use crate::contract::tests::declaration_serving;
use crate::contract::{FeedError, MarketFeed, Offered};
use crate::profile::DataClass;

const RECEIVED: i128 = 1_787_469_688_200_000_000;

fn level(price_scaled: i128, quantity_scaled: i128) -> BookLevel {
    BookLevel { price_scaled, quantity_scaled }
}

fn record(fact: MarketFact) -> MarketRecord {
    MarketRecord {
        instrument: VenueSymbol::new("BTCUSDT"),
        exchange_time: ExchangeTime::from_ms(1_787_469_688_129),
        received_at_ns: RECEIVED,
        fact,
    }
}

fn decimal(value_scaled: i128, scale: u8) -> ScaledDecimal {
    ScaledDecimal { value_scaled, scale }
}

fn snapshot(bids: Vec<BookLevel>, asks: Vec<BookLevel>) -> MarketFact {
    MarketFact::BookSnapshot(BookSnapshot { sequence: Sequence::Numbered(9), bids, asks })
}

/// One well-formed record of every fact, so each class is proven to pass before any is refused.
fn one_of_each() -> Vec<MarketFact> {
    vec![
        snapshot(vec![level(101, 1), level(100, 2)], vec![level(102, 1), level(103, 4)]),
        MarketFact::BookDelta(BookDelta {
            sequence: Sequence::Numbered(10),
            changes: vec![
                LevelChange::Set { side: BookSide::Bid, price_scaled: 101, quantity_scaled: 3 },
                LevelChange::Remove { side: BookSide::Ask, price_scaled: 102 },
            ],
        }),
        MarketFact::Trade(Trade {
            trade_id: "t-1".into(),
            aggressor: Aggressor::Buyer,
            price_scaled: 101,
            quantity_scaled: 5,
        }),
        MarketFact::Liquidation(Liquidation {
            forced_order_side: Side::Sell,
            price_scaled: 99,
            quantity_scaled: 5,
        }),
        MarketFact::Funding(Funding {
            rate: decimal(-125, 6),
            next_funding_time: ExchangeTime::NotPublished,
        }),
        MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled: 0 }),
        MarketFact::MarkPrice { mark_price_scaled: 100 },
        MarketFact::IndexPrice { index_price_scaled: 100 },
        MarketFact::LongShortRatio(LongShortRatio {
            long_share: decimal(55, 2),
            short_share: decimal(45, 2),
            period_ns: 300_000_000_000,
        }),
    ]
}

#[test]
fn a_well_formed_record_of_every_fact_passes() {
    for fact in one_of_each() {
        assert_eq!(check_record(&record(fact.clone())), Ok(()), "{fact:?}");
    }
}

/// Every class a record can belong to is a class a profile can declare, and no two facts
/// that differ in kind share a class except the two halves of a book.
#[test]
fn each_fact_belongs_to_the_class_a_profile_declares_for_it() {
    let classes: Vec<DataClass> = one_of_each().iter().map(class_of).collect();
    assert_eq!(
        classes,
        vec![
            DataClass::Depth,
            DataClass::Depth,
            DataClass::Trades,
            DataClass::Liquidations,
            DataClass::Funding,
            DataClass::OpenInterest,
            DataClass::MarkPrice,
            DataClass::IndexPrice,
            DataClass::LongShortRatio,
        ]
    );
}

#[test]
fn the_venue_stamp_and_the_arrival_are_both_required_to_be_real() {
    let mut r = record(MarketFact::MarkPrice { mark_price_scaled: 1 });
    r.exchange_time = ExchangeTime::At { at_ns: 0 };
    assert!(matches!(check_record(&r), Err(RecordDefect::ExchangeTimeNotPositive { .. })));

    let mut r = record(MarketFact::MarkPrice { mark_price_scaled: 1 });
    r.received_at_ns = -1;
    assert_eq!(check_record(&r), Err(RecordDefect::ReceivedBeforeEpoch { received_at_ns: -1 }));

    let mut r = record(MarketFact::MarkPrice { mark_price_scaled: 1 });
    r.instrument = VenueSymbol::new("");
    assert_eq!(check_record(&r), Err(RecordDefect::NoInstrument));
}

#[test]
fn an_unstamped_fact_is_stated_as_unstamped_and_passes() {
    let mut r = record(MarketFact::MarkPrice { mark_price_scaled: 1 });
    r.exchange_time = ExchangeTime::NotPublished;
    assert_eq!(check_record(&r), Ok(()));
}

#[test]
fn a_millisecond_stamp_widens_exactly() {
    assert_eq!(ExchangeTime::from_ms(1_500), ExchangeTime::At { at_ns: 1_500_000_000 });
}

/// A side that is not best first would make the first level something other than the best.
#[test]
fn a_snapshot_side_out_of_order_is_refused() {
    let bids_rising = snapshot(vec![level(100, 1), level(101, 1)], vec![]);
    assert_eq!(
        check_record(&record(bids_rising)),
        Err(RecordDefect::SideOutOfOrder { side: BookSide::Bid, index: 1 })
    );

    let ask_twice = snapshot(vec![], vec![level(102, 1), level(102, 2)]);
    assert_eq!(
        check_record(&record(ask_twice)),
        Err(RecordDefect::SideOutOfOrder { side: BookSide::Ask, index: 1 })
    );
}

#[test]
fn a_level_with_no_quantity_is_refused_wherever_it_claims_to_be_a_level() {
    let empty_level = snapshot(vec![level(100, 0)], vec![]);
    assert!(matches!(check_record(&record(empty_level)), Err(RecordDefect::NotPositive { .. })));

    let empty_set = MarketFact::BookDelta(BookDelta {
        sequence: Sequence::NotNumbered,
        changes: vec![LevelChange::Set {
            side: BookSide::Ask,
            price_scaled: 100,
            quantity_scaled: 0,
        }],
    });
    assert!(matches!(check_record(&record(empty_set)), Err(RecordDefect::NotPositive { .. })));
}

#[test]
fn a_trade_without_identity_cannot_be_deduplicated_and_is_refused() {
    let anonymous = MarketFact::Trade(Trade {
        trade_id: String::new(),
        aggressor: Aggressor::NotPublished,
        price_scaled: 100,
        quantity_scaled: 1,
    });
    assert_eq!(check_record(&record(anonymous)), Err(RecordDefect::NoTradeIdentity));
}

#[test]
fn figures_outside_their_domain_are_refused() {
    let refused = [
        MarketFact::MarkPrice { mark_price_scaled: 0 },
        MarketFact::IndexPrice { index_price_scaled: -1 },
        MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled: -1 }),
        MarketFact::OpenInterest(OpenInterest::QuoteValue { value: decimal(1, 39) }),
        MarketFact::Funding(Funding {
            rate: decimal(1, 39),
            next_funding_time: ExchangeTime::NotPublished,
        }),
        MarketFact::LongShortRatio(LongShortRatio {
            long_share: decimal(55, 2),
            short_share: decimal(45, 2),
            period_ns: 0,
        }),
        MarketFact::Liquidation(Liquidation {
            forced_order_side: Side::Buy,
            price_scaled: 1,
            quantity_scaled: 0,
        }),
    ];
    for fact in refused {
        assert!(check_record(&record(fact.clone())).is_err(), "{fact:?} passed");
    }
}

/// Replays fixed answers. The faults are chosen by the test.
struct ScriptedFeed {
    answers: Vec<Result<Offered, FeedError>>,
}

impl MarketFeed for ScriptedFeed {
    fn offer(&mut self, _frame: &[u8], _received_at_ns: i128) -> Result<Offered, FeedError> {
        if self.answers.is_empty() { Ok(Offered::default()) } else { self.answers.remove(0) }
    }

    fn reset(&mut self) {}

    fn accepted(&self) -> u64 {
        0
    }

    fn instruments(&self) -> usize {
        0
    }
}

fn frames() -> Vec<RecordedFrame<'static>> {
    vec![RecordedFrame { received_at_ns: RECEIVED, frame: b"{}" }]
}

fn run(answer: Result<Offered, FeedError>, classes: Vec<DataClass>) -> Vec<CheckResult> {
    let mut feed = ScriptedFeed { answers: vec![answer] };
    check_feed(&mut feed, &frames(), &declaration_serving(classes))
}

fn failed(results: &[CheckResult]) -> Vec<&'static str> {
    results
        .iter()
        .filter(|r| matches!(r.outcome, CheckOutcome::Failed { .. }))
        .map(|r| r.name)
        .collect()
}

fn offered(record: MarketRecord) -> Result<Offered, FeedError> {
    Ok(Offered { records: vec![record], top: None })
}

#[test]
fn a_feed_that_keeps_the_contract_passes_every_feed_check() {
    let book = record(one_of_each().remove(0));
    let results = run(offered(book), vec![DataClass::Depth]);
    assert_eq!(results.len(), 5);
    assert!(failed(&results).is_empty(), "{results:?}");
}

#[test]
fn a_feed_that_emits_nothing_does_not_pass_by_saying_nothing() {
    assert_eq!(failed(&run(Ok(Offered::default()), vec![])), vec!["records-observed"]);
}

#[test]
fn a_feed_that_restamps_arrival_is_caught() {
    let mut late = record(MarketFact::MarkPrice { mark_price_scaled: 1 });
    late.received_at_ns = RECEIVED + 1;
    assert_eq!(
        failed(&run(offered(late), vec![DataClass::MarkPrice])),
        vec!["records-stamped-on-arrival"]
    );
}

/// Undeclared data is unmonitored data: nothing holds it to a staleness bound.
#[test]
fn a_feed_that_emits_an_undeclared_class_is_caught() {
    let trade = record(one_of_each().remove(2));
    assert_eq!(failed(&run(offered(trade), vec![DataClass::Depth])), vec!["records-declared"]);
}

#[test]
fn a_feed_that_emits_a_malformed_record_is_caught() {
    let zero = record(MarketFact::MarkPrice { mark_price_scaled: 0 });
    assert_eq!(
        failed(&run(offered(zero), vec![DataClass::MarkPrice])),
        vec!["records-well-formed"]
    );
}

#[test]
fn a_feed_that_refuses_a_recorded_frame_is_caught() {
    let refused = Err(FeedError::Unreadable { detail: "x".into() });
    let names = failed(&run(refused, vec![]));
    assert_eq!(names, vec!["recording-accepted", "records-observed"]);
}
