// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One venue link turned into normalised events.

use bijective_venue::contract::{
    ConnectionState, FeedError, MarketEvent, MarketFeed, Offered, ReceivedFrame, StreamRequest,
};
use tokio::sync::mpsc;

use crate::feed::Feed;
use crate::transport::{Connection, Signal};

pub async fn run(request: &StreamRequest, out: mpsc::Sender<MarketEvent>) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel::<Signal>(4096);
    let scales = request.scales.uniform().map_err(|e| e.to_string())?;
    let connection =
        Connection::new(request, scales).map_err(|e| format!("session refused: {e}"))?;
    let driver = tokio::spawn(connection.run(tx));

    let mut feed = Feed::new(scales.price, scales.quantity);

    while let Some(signal) = rx.recv().await {
        match signal {
            Signal::Frame { received_at_ns, frame, parsed } => {
                let received = ReceivedFrame { received_at_ns, frame: frame.into_bytes() };
                if out.send(MarketEvent::Received(received)).await.is_err() {
                    break;
                }
                // The same reading as `offer`, so a replay of the received frame decides alike.
                let observed = parsed
                    .map_err(|e| FeedError::Unreadable { detail: e.to_string() })
                    .and_then(|message| feed.observe(&message, received_at_ns));
                match observed {
                    Ok(offered) => {
                        if !forward(&out, offered).await {
                            break;
                        }
                    }
                    Err(e) => {
                        if out.send(MarketEvent::Refused(e)).await.is_err() {
                            break;
                        }
                    }
                }
            }

            Signal::State(state) => {
                // Any state but connected drops the books: nothing joins across a reconnect on this
                // venue. The consumer hears of the gap before the next observation.
                if !matches!(state, ConnectionState::Connected { .. }) {
                    feed.reset();
                    if out.send(MarketEvent::ContinuityLost).await.is_err() {
                        break;
                    }
                }
                if out.send(MarketEvent::State(state)).await.is_err() {
                    break;
                }
            }
        }
    }

    driver.abort();
    Ok(())
}

/// Records first, then the top they produced. False when the consumer has gone.
async fn forward(out: &mpsc::Sender<MarketEvent>, offered: Offered) -> bool {
    for record in offered.records {
        if out.send(MarketEvent::Record(record)).await.is_err() {
            return false;
        }
    }
    match offered.top {
        Some(top) => out.send(MarketEvent::Observation(top)).await.is_ok(),
        None => true,
    }
}
