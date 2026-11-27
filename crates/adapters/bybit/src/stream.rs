// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Live path assembled inside the adapter: parse once, normalise, emit.
//! The component above must not know the venue wire format.

use bijective_venue::contract::{
    ConnectionState as ContractState, DegradedReason, FeedError, MarketEvent, MarketFeed, Moment,
    Offered, ReceivedFrame, StreamRequest,
};
use tokio::sync::mpsc;

use crate::feed::Feed;
use crate::message::MAX_TOPICS_PER_CONNECTION;
use crate::session::{ConnectionState, Degraded};
use crate::transport::{Connection, Notice, Signal};

/// Run one connection's worth of instruments, emitting normalised events until the consumer stops
/// listening or the session gives up.
pub async fn run_market_stream(
    request: &StreamRequest,
    out: mpsc::Sender<MarketEvent>,
) -> Result<(), String> {
    let topics = topics_for(&request.endpoint, &request.instruments);
    if topics.len() > MAX_TOPICS_PER_CONNECTION {
        return Err(format!(
            "{} topics asked of a connection that carries {MAX_TOPICS_PER_CONNECTION}",
            topics.len()
        ));
    }

    let requested = topics.len();
    let (sig_tx, mut sig_rx) = mpsc::channel::<Signal>(4096);
    let (notice_tx, notice_rx) = mpsc::channel::<Notice>(8);

    let connection =
        Connection::new(request, topics).map_err(|e| format!("session refused: {e}"))?;
    let driver = tokio::spawn(connection.run(sig_tx, notice_rx));

    let mut feed = Feed::new(request.scales.clone());

    while let Some(signal) = sig_rx.recv().await {
        match signal {
            Signal::Frame { received_at_ns, frame, parsed } => {
                let received = ReceivedFrame { received_at_ns, frame: frame.into_bytes() };
                if out.send(MarketEvent::Received(received)).await.is_err() {
                    break;
                }
                let message = match parsed {
                    Ok(message) => message,
                    Err(e) => {
                        let refused = FeedError::Unreadable { detail: e.to_string() };
                        if out.send(MarketEvent::Refused(refused)).await.is_err() {
                            break;
                        }
                        continue;
                    }
                };
                match feed.observe(&message, received_at_ns) {
                    Ok(offered) => {
                        if !forward(&out, offered).await {
                            break;
                        }
                    }
                    Err(e) => {
                        // Drop books and tell the consumer before the socket rebuilds, so a gap is never learned late.
                        feed.reset();
                        if out.send(MarketEvent::Refused(e)).await.is_err()
                            || out.send(MarketEvent::ContinuityLost).await.is_err()
                        {
                            break;
                        }
                        let _ = notice_tx.try_send(Notice::SequenceGap);
                    }
                }
            }

            Signal::State(state) => {
                if drops_books(&state) {
                    feed.reset();
                    if out.send(MarketEvent::ContinuityLost).await.is_err() {
                        break;
                    }
                }
                if out.send(MarketEvent::State(translate(&state, requested))).await.is_err() {
                    break;
                }
            }
        }
    }

    driver.abort();
    Ok(())
}

/// The topics one connection carries for `instruments`. Every endpoint carries the book and the
/// trades. A derivatives endpoint (linear or inverse) adds the ticker, which holds funding, open
/// interest, mark and index, and the liquidation stream. Spot has neither, and a topic the venue
/// refuses would degrade the whole connection.
pub(crate) fn topics_for(endpoint: &str, instruments: &[String]) -> Vec<String> {
    let derivatives = endpoint.contains("/linear") || endpoint.contains("/inverse");
    let mut topics = Vec::new();
    for symbol in instruments {
        topics.push(format!("orderbook.50.{symbol}"));
        topics.push(format!("publicTrade.{symbol}"));
        if derivatives {
            topics.push(format!("tickers.{symbol}"));
            topics.push(format!("allLiquidation.{symbol}"));
        }
    }
    topics
}

/// Records first, then the top they produced. False when the consumer has gone.
pub(crate) async fn forward(out: &mpsc::Sender<MarketEvent>, offered: Offered) -> bool {
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

/// Does leaving this state mean the books we hold are no longer describable by the venue?
fn drops_books(state: &ConnectionState) -> bool {
    matches!(state, ConnectionState::Degraded(_) | ConnectionState::Failed(_))
}

/// Session states onto the contract's. `Subscribing` is `Connecting`. Instants are `NotTracked`.
pub(crate) fn translate(state: &ConnectionState, requested: usize) -> ContractState {
    match state {
        ConnectionState::Down | ConnectionState::Failed(_) => ContractState::Disconnected,
        ConnectionState::Connecting | ConnectionState::Subscribing => ContractState::Connecting,
        ConnectionState::Live => ContractState::Connected { since: Moment::NotTracked },
        ConnectionState::Degraded(Degraded::LinkSilent) => ContractState::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::Stale { last_message: Moment::NotTracked },
        },
        ConnectionState::Degraded(Degraded::SubscribeRefused | Degraded::SubscribeUnanswered) => {
            ContractState::Degraded {
                since: Moment::NotTracked,
                // A refusal attaches nothing, so attached is 0.
                reason: DegradedReason::SubscriptionsIncomplete { attached: 0, requested },
            }
        }
        ConnectionState::Degraded(_) => ContractState::Disconnected,
    }
}
