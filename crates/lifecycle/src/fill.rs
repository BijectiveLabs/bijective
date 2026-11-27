// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A fill: one execution of part of an order, as the venue reported it.

use bijective_model::Side;
use bijective_model::market::ExchangeTime;

/// Which side of the book the execution took, where the venue says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liquidity {
    Maker,
    Taker,
    NotPublished,
}

/// A fee in the asset the venue charged it in. Signed: positive is paid, negative is a rebate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fee {
    pub asset: String,
    pub amount_scaled: i128,
    pub scale: u8,
}

/// Invariant enforced where a fill is applied: positive price and quantity, a non-empty identity.
/// The identity is the venue's execution identifier. A fill delivered twice is recognised by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    pub fill_id: String,
    pub venue_order_id: Option<String>,
    pub side: Side,
    pub price_scaled: i128,
    /// Base quantity at the instrument's quantity scale.
    pub quantity_scaled: i128,
    pub fee: Option<Fee>,
    pub liquidity: Liquidity,
    pub exchange_time: ExchangeTime,
    pub received_at_ns: i128,
}
