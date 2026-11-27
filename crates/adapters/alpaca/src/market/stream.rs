// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The socket and the run loop. Every decision belongs to `session`. Thinnest part of the
//! adapter, because it cannot be tested without a peer.

use std::time::{Duration, Instant};

use bijective_secrets_broker::Credential;
use bijective_venue::contract::{
    ConnectionState, FeedError, MarketEvent, Offered, ReceivedFrame, StreamRequest,
};
use bijective_venue::resilience::SplitMix64;
use bijective_venue_socket::{Inbound as Received, Limits, Socket};
use tokio::sync::mpsc;

use super::feed::Feed;
use super::message::{auth_frame, parse};
use super::session::{LinkAction, LinkEvent, LinkSession};

/// Every frame here is a few kilobytes. The library default of 64 MiB would let a peer hold that
/// much memory.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

type WsStream = Socket;

enum Arrival {
    Tick,
    Closed,
    Pong,
    Text(String),
}

/// The bounds every connection to the venue opens with.
fn socket_limits(connect_timeout: std::time::Duration) -> Limits {
    Limits { connect_timeout, max_message_bytes: MAX_MESSAGE_BYTES }
}

/// Runs the stream until the consumer goes away. The key id and secret key authenticate the
/// link. The secret is not kept past the authentication message.
pub async fn run(
    request: &StreamRequest,
    key_id: &str,
    secret: &Credential,
    out: mpsc::Sender<MarketEvent>,
) -> Result<(), String> {
    let secret = String::from_utf8(secret.expose().to_vec())
        .map_err(|_| "the secret key is not text".to_string())?;
    let mut session = LinkSession::new(
        auth_frame(key_id, &secret),
        request.instruments.clone(),
        0,
        Box::new(SplitMix64::for_this_process(request.clock.now_ns())),
    );
    let scales = request.scales.uniform().map_err(|e| e.to_string())?;
    let mut feed = Feed::new(scales.price, scales.quantity);
    let base = Instant::now();
    let mut socket: Option<WsStream> = None;
    loop {
        let now = elapsed_ns(base);
        let sleep_for = session.wake_at().map_or(Duration::from_secs(1), |w| {
            Duration::from_nanos(u64::try_from((w - now).max(0)).unwrap_or(u64::MAX))
        });
        let arrival = tokio::select! {
            frame = read_frame(&mut socket) => frame,
            () = tokio::time::sleep(sleep_for) => Some(Arrival::Tick),
        };
        let Some(arrival) = arrival else { continue };
        let received_at_ns = request.clock.now_ns();
        let now = elapsed_ns(base);
        let actions = match arrival {
            Arrival::Tick => session.step(LinkEvent::Tick, now),
            Arrival::Pong => session.step(LinkEvent::Pong, now),
            Arrival::Closed => {
                socket = None;
                session.step(LinkEvent::SocketClosed, now)
            }
            Arrival::Text(text) => {
                let received = ReceivedFrame { received_at_ns, frame: text.clone().into_bytes() };
                if out.send(MarketEvent::Received(received)).await.is_err() {
                    return Ok(());
                }
                match parse(text.as_bytes(), scales.price, scales.quantity) {
                    Ok(messages) => {
                        let observed = feed.observe(&messages, received_at_ns);
                        if !forward(&out, observed).await {
                            return Ok(());
                        }
                        session.step(LinkEvent::Inbound(&messages), now)
                    }
                    Err(e) => {
                        let refused =
                            MarketEvent::Refused(FeedError::Unreadable { detail: e.to_string() });
                        if out.send(refused).await.is_err() {
                            return Ok(());
                        }
                        session.step(LinkEvent::Unreadable, now)
                    }
                }
            }
        };
        if !carry_out(actions, &mut session, &mut socket, &out, &request.endpoint, base).await {
            return Ok(());
        }
    }
}

async fn carry_out(
    actions: Vec<LinkAction>,
    session: &mut LinkSession,
    socket: &mut Option<WsStream>,
    out: &mpsc::Sender<MarketEvent>,
    url: &str,
    base: Instant,
) -> bool {
    let mut queue = std::collections::VecDeque::from(actions);
    while let Some(action) = queue.pop_front() {
        let follow = match action {
            LinkAction::Report(state) => {
                if !matches!(state, ConnectionState::Connected { .. }) {
                    if let Some(refusal) = session.refusal.take() {
                        eprintln!("market stream refused by the venue: {refusal}");
                    }
                    if out.send(MarketEvent::ContinuityLost).await.is_err() {
                        return false;
                    }
                }
                if out.send(MarketEvent::State(state)).await.is_err() {
                    return false;
                }
                continue;
            }
            LinkAction::Connect => {
                let limits = socket_limits(Duration::from_secs(10));
                let event = match bijective_venue_socket::connect(url, limits).await {
                    Ok(ws) => {
                        *socket = Some(ws);
                        LinkEvent::Connected
                    }
                    Err(_) => LinkEvent::ConnectFailed,
                };
                session.step(event, elapsed_ns(base))
            }
            LinkAction::Send(frame) => send(socket, Outbound::Text(frame), session, base).await,
            LinkAction::Ping => send(socket, Outbound::Ping, session, base).await,
            LinkAction::Disconnect => {
                if let Some(mut ws) = socket.take() {
                    ws.close().await;
                }
                Vec::new()
            }
        };
        queue.extend(follow);
    }
    true
}

/// What the link sends: a text frame, or a ping whose pong shows the link is alive.
enum Outbound {
    Text(String),
    Ping,
}

async fn send(
    socket: &mut Option<WsStream>,
    message: Outbound,
    session: &mut LinkSession,
    base: Instant,
) -> Vec<LinkAction> {
    let sent = match (socket.as_mut(), message) {
        (Some(ws), Outbound::Text(text)) => ws.send_text(text).await.is_ok(),
        (Some(ws), Outbound::Ping) => ws.send_ping().await.is_ok(),
        (None, _) => false,
    };
    if sent {
        return Vec::new();
    }
    *socket = None;
    session.step(LinkEvent::SocketClosed, elapsed_ns(base))
}

/// Records first, then the top they produced. False when the consumer has gone.
async fn forward(out: &mpsc::Sender<MarketEvent>, observed: Result<Offered, FeedError>) -> bool {
    let offered = match observed {
        Ok(offered) => offered,
        Err(e) => return out.send(MarketEvent::Refused(e)).await.is_ok(),
    };
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

/// Waits forever without a socket: the timer arm is the only way a connection is made.
async fn read_frame(socket: &mut Option<WsStream>) -> Option<Arrival> {
    let Some(ws) = socket.as_mut() else {
        return std::future::pending().await;
    };
    match ws.next().await {
        Received::Text(text) => Some(Arrival::Text(text)),
        Received::Pong => Some(Arrival::Pong),
        Received::Binary(_) => None,
        Received::Closed => Some(Arrival::Closed),
    }
}

fn elapsed_ns(base: Instant) -> i128 {
    i128::try_from(base.elapsed().as_nanos()).unwrap_or(i128::MAX)
}
