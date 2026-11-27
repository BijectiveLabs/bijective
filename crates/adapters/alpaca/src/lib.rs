// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Alpaca venue adapter for US equities: the market data stream, and the execution side over
//! its Trading API.
//! Venue sources and behaviour: `docs/integrations/alpaca.md`. The asset class:
//! `docs/decisions/0004-multi-asset-equities.md`.

pub mod evidence;
pub mod market;
pub mod order;
pub mod rest;
pub mod trade_session;
pub mod trade_stream;

pub use market::Feed;
pub use market::stream::run as run_market_stream;

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
