// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The `fact` event: a trade, liquidation, funding rate, open interest, mark or index price,
//! long/short ratio or resolution, as a strategy reads it.
//!
//! One event names every kind (`kind` says which), so a kind the platform adds later reaches a
//! strategy library as a `fact` it can pass over, not as an event it cannot read.

use bijective_model::Side;
use bijective_model::market::{Aggressor, ExchangeTime, MarketFact, OpenInterest, ScaledDecimal};
use bijective_serialization::signal::facts::FactUpdate;
use serde::{Deserialize, Serialize};

use super::{Inbound, Wide};

/// What one fact states, by kind. Figures are scaled integers as text, at the instrument's scales
/// unless a `_scale` beside them says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FactValues {
    Trade {
        trade_id: String,
        /// `buyer` or `seller`. Absent where the venue does not say which side took liquidity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aggressor: Option<String>,
        price_scaled: Wide,
        quantity_scaled: Wide,
    },
    /// The side is that of the forced order, not of the position it closed.
    Liquidation {
        forced_order_side: String,
        price_scaled: Wide,
        quantity_scaled: Wide,
    },
    /// Per funding interval, signed: positive means longs pay shorts.
    Funding {
        rate_scaled: Wide,
        rate_scale: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_funding_ns: Option<Wide>,
    },
    /// In the unit the venue publishes: a base quantity, or a quote value with its own scale.
    OpenInterest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_quantity_scaled: Option<Wide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote_value_scaled: Option<Wide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote_value_scale: Option<u8>,
    },
    MarkPrice {
        mark_price_scaled: Wide,
    },
    IndexPrice {
        index_price_scaled: Wide,
    },
    LongShortRatio {
        long_share_scaled: Wide,
        long_share_scale: u8,
        short_share_scaled: Wide,
        short_share_scale: u8,
        period_ns: Wide,
    },
    /// The outcome's question resolved: `won` redeems for one unit of collateral, a loss for none.
    Resolution {
        won: bool,
    },
}

/// The line for the fact at `fact_position` on a facts stream. `None` for a book snapshot or
/// delta, which a facts stream never holds.
#[must_use]
pub fn fact_line(fact_position: u64, update: &FactUpdate, venue: Option<&str>) -> Option<Inbound> {
    Some(Inbound::Fact {
        fact_position: fact_position.to_string(),
        instrument: update.instrument.as_str().to_string(),
        venue: venue.map(str::to_string),
        exchange_ts_ns: match update.exchange_time {
            ExchangeTime::At { at_ns } => Some(at_ns.to_string()),
            ExchangeTime::NotPublished => None,
        },
        received_at_ns: update.received_at_ns.to_string(),
        values: values_of(&update.fact)?,
    })
}

fn values_of(fact: &MarketFact) -> Option<FactValues> {
    Some(match fact {
        MarketFact::Trade(t) => FactValues::Trade {
            trade_id: t.trade_id.clone(),
            aggressor: match t.aggressor {
                Aggressor::Buyer => Some("buyer".to_string()),
                Aggressor::Seller => Some("seller".to_string()),
                Aggressor::NotPublished => None,
            },
            price_scaled: t.price_scaled.to_string(),
            quantity_scaled: t.quantity_scaled.to_string(),
        },
        MarketFact::Liquidation(l) => FactValues::Liquidation {
            forced_order_side: match l.forced_order_side {
                Side::Buy => "buy".to_string(),
                Side::Sell => "sell".to_string(),
            },
            price_scaled: l.price_scaled.to_string(),
            quantity_scaled: l.quantity_scaled.to_string(),
        },
        MarketFact::Funding(f) => FactValues::Funding {
            rate_scaled: f.rate.value_scaled.to_string(),
            rate_scale: f.rate.scale,
            next_funding_ns: match f.next_funding_time {
                ExchangeTime::At { at_ns } => Some(at_ns.to_string()),
                ExchangeTime::NotPublished => None,
            },
        },
        MarketFact::OpenInterest(oi) => open_interest(*oi),
        MarketFact::MarkPrice { mark_price_scaled } => {
            FactValues::MarkPrice { mark_price_scaled: mark_price_scaled.to_string() }
        }
        MarketFact::IndexPrice { index_price_scaled } => {
            FactValues::IndexPrice { index_price_scaled: index_price_scaled.to_string() }
        }
        MarketFact::LongShortRatio(r) => FactValues::LongShortRatio {
            long_share_scaled: r.long_share.value_scaled.to_string(),
            long_share_scale: r.long_share.scale,
            short_share_scaled: r.short_share.value_scaled.to_string(),
            short_share_scale: r.short_share.scale,
            period_ns: r.period_ns.to_string(),
        },
        MarketFact::Resolution { won } => FactValues::Resolution { won: *won },
        MarketFact::BookSnapshot(_) | MarketFact::BookDelta(_) => return None,
    })
}

fn open_interest(oi: OpenInterest) -> FactValues {
    match oi {
        OpenInterest::BaseQuantity { quantity_scaled } => FactValues::OpenInterest {
            base_quantity_scaled: Some(quantity_scaled.to_string()),
            quote_value_scaled: None,
            quote_value_scale: None,
        },
        OpenInterest::QuoteValue { value: ScaledDecimal { value_scaled, scale } } => {
            FactValues::OpenInterest {
                base_quantity_scaled: None,
                quote_value_scaled: Some(value_scaled.to_string()),
                quote_value_scale: Some(scale),
            }
        }
    }
}
