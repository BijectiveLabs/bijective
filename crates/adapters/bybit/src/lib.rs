// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Bybit venue adapter. Design notes: `docs/integrations/bybit.md`.

pub mod book;
pub mod evidence;
pub mod execution;
pub mod feed;
pub mod market;
pub mod message;
pub mod observe;
pub mod private;
pub mod private_session;
pub mod private_stream;
pub mod rest;
pub mod session;
pub mod sign;
pub mod stream;
pub mod transport;

pub use book::{BookError, L2Book, Level};
pub use feed::Feed;
pub use message::{BookUpdateKind, ParseError, VenueMessage, parse};
pub use session::{Action, ConnectionState, Degraded, Event, Failure, Session, SessionError};
pub use stream::run_market_stream;
pub use transport::{Captured, Connection, Notice, Signal};

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
