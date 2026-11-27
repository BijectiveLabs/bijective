// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Polymarket venue adapter: the market stream of its central limit order book, and the signed
//! execution side over its REST interface.
//! Venue sources and behaviour: `docs/integrations/polymarket.md`. The instrument kind:
//! `docs/decisions/0002-prediction-market-outcomes.md`.

pub mod evidence;
pub mod feed;
pub mod message;
pub mod order;
pub mod rest;
pub mod session;
pub mod signing;
pub mod stream;
pub mod transport;

pub use feed::Feed;
pub use stream::run as run_market_stream;

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
