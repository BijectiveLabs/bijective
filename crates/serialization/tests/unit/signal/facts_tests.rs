// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use bijective_model::Sequence;
use bijective_model::market::{BookSnapshot, MarketFact};

fn update(fact: MarketFact) -> FactUpdate {
    FactUpdate {
        instrument: InstrumentId::new("btc-perp"),
        exchange_time: ExchangeTime::At { at_ns: 1_788_000_000_000_000_000 },
        received_at_ns: 1_788_000_000_001_000_000,
        fact,
    }
}

fn rate(value_scaled: i128) -> ScaledDecimal {
    ScaledDecimal { value_scaled, scale: 8 }
}

/// Every kind a facts stream carries reads back as written, and encodes the same way twice.
#[test]
fn every_kind_of_fact_round_trips() {
    let facts = [
        MarketFact::Trade(Trade {
            trade_id: "t-1".into(),
            aggressor: Aggressor::Seller,
            price_scaled: 2_654_375,
            quantity_scaled: 1_500,
        }),
        MarketFact::Liquidation(Liquidation {
            forced_order_side: Side::Sell,
            price_scaled: 2_600_000,
            quantity_scaled: 10,
        }),
        MarketFact::Funding(Funding {
            rate: rate(-1_000),
            next_funding_time: ExchangeTime::NotPublished,
        }),
        MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled: 9 }),
        MarketFact::OpenInterest(OpenInterest::QuoteValue { value: rate(7) }),
        MarketFact::MarkPrice { mark_price_scaled: 2_654_400 },
        MarketFact::IndexPrice { index_price_scaled: 2_654_300 },
        MarketFact::LongShortRatio(LongShortRatio {
            long_share: rate(55),
            short_share: rate(45),
            period_ns: 300_000_000_000,
        }),
        MarketFact::Resolution { won: true },
        MarketFact::Resolution { won: false },
    ];
    for fact in facts {
        let written = update(fact);
        let bytes = encode(&written).expect("encode");
        assert_eq!(bytes, encode(&written).expect("encode"));
        assert_eq!(decode(&bytes).expect("decode"), written);
    }
}

/// A book belongs on the market stream. Writing one here would send its depth twice.
#[test]
fn a_book_is_not_a_fact() {
    let book = MarketFact::BookSnapshot(BookSnapshot {
        sequence: Sequence::NotNumbered,
        bids: Vec::new(),
        asks: Vec::new(),
    });
    assert!(!is_fact(&book));
    assert!(matches!(encode(&update(book)), Err(FactError::NotAFact)));
}

#[test]
fn each_venue_has_its_own_facts_stream() {
    assert_eq!(venue_of(&stream("venue-a")), Some("venue-a"));
    assert_eq!(venue_of("market-venue-a"), None);
}
