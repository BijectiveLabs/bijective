// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The stock market data stream: quotes and trades for the subscribed symbols, behind a key id
//! and secret key (Streaming market data). Sources: `docs/integrations/alpaca.md`.

pub mod feed;
pub mod message;
pub mod session;
pub mod stream;

pub use feed::Feed;

#[cfg(test)]
#[path = "../../tests/unit/market_tests.rs"]
mod tests;
