// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Hyperliquid venue adapter.
//! Venue sources and behaviour: `docs/integrations/hyperliquid.md`.

pub mod evidence;
pub mod feed;
pub mod message;
pub mod observe;
pub mod session;
pub mod stream;
pub mod transport;
pub mod user_session;
pub mod user_stream;
// Private: no signing surface may be reachable from other crates.
mod exchange;
pub mod rest;
mod signing;
mod wire;

pub use feed::Feed;
pub use stream::run as run_market_stream;

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
