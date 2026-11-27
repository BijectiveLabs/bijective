// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The trade updates stream's socket, run on a thread of its own so the node's pass cadence never
//! starves its pings. It holds no order state: it raises a flag when the venue reports a change,
//! and the venue evidence read stays the one source of order states and fills.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bijective_common::clock::Clock;
use bijective_venue::contract::Pushed;
use bijective_venue::resilience::SplitMix64;
use bijective_venue_socket::{Inbound as Received, Limits, Socket};

use crate::market::message::auth_frame;
use crate::trade_session::{
    ANSWER_TIMEOUT_NS, TradeAction, TradeEvent, TradeSession, TradeState, classify,
};

/// The longest the driver waits before it looks at the stop flag again.
const STOP_POLL: Duration = Duration::from_secs(1);

/// The stream on the host the venue pairs with each Trading API host (WebSocket streaming, read
/// 2026-11-17). Any other endpoint has none this build knows, and is read on the evidence cadence.
#[must_use]
pub fn endpoint_for(rest_endpoint: &str) -> Option<&'static str> {
    match rest_endpoint.trim_end_matches('/') {
        "https://api.alpaca.markets" => Some("wss://api.alpaca.markets/stream"),
        "https://paper-api.alpaca.markets" => Some("wss://paper-api.alpaca.markets/stream"),
        _ => None,
    }
}

/// What the link needs. The authentication message is built here once. It carries the secret.
pub struct LinkSetup {
    pub url: String,
    pub key_id: String,
    pub secret: String,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, Default)]
struct Shared {
    changed: AtomicBool,
    live: AtomicBool,
    stop: AtomicBool,
}

/// The running link. Dropping it stops the thread at its next look at the stop flag.
#[derive(Debug)]
pub struct TradeLink {
    shared: Arc<Shared>,
}

impl TradeLink {
    /// Starts the link's thread. An `Err` is the operating system refusing a thread.
    pub fn start(setup: LinkSetup) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let held = Arc::clone(&shared);
        std::thread::Builder::new().name("alpaca-trades".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build()
            else {
                return;
            };
            runtime.block_on(run(setup, held));
        })?;
        Ok(Self { shared })
    }

    /// What the link said since the last ask. A report outranks the link's state.
    pub fn pushed(&self) -> Pushed {
        if self.shared.changed.swap(false, Ordering::AcqRel) {
            Pushed::Changed
        } else if self.shared.live.load(Ordering::Acquire) {
            Pushed::Quiet
        } else {
            Pushed::Lapsed
        }
    }
}

impl Drop for TradeLink {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

type WsStream = Socket;

enum Inbound {
    Tick,
    Closed,
    Pong,
    /// Text or binary: the venue's paper stream sends binary frames holding JSON.
    Frame(Vec<u8>),
}

async fn run(setup: LinkSetup, shared: Arc<Shared>) {
    let base = Instant::now();
    let auth = auth_frame(&setup.key_id, &setup.secret);
    let jitter = Box::new(SplitMix64::for_this_process(setup.clock.now_ns()));
    let mut session = TradeSession::new(0, jitter);
    let mut socket: Option<WsStream> = None;
    while !shared.stop.load(Ordering::Acquire) {
        let now = elapsed_ns(base);
        let wait = session
            .wake_at()
            .map(|w| Duration::from_nanos(u64::try_from((w - now).max(0)).unwrap_or(u64::MAX)))
            .unwrap_or(STOP_POLL)
            .min(STOP_POLL);
        let inbound = tokio::select! {
            frame = read_frame(&mut socket) => frame,
            () = tokio::time::sleep(wait) => Some(Inbound::Tick),
        };
        let Some(inbound) = inbound else { continue };
        let now = elapsed_ns(base);
        let actions = match inbound {
            Inbound::Tick => session.step(TradeEvent::Tick, now),
            Inbound::Pong => session.step(TradeEvent::Pong, now),
            Inbound::Closed => {
                socket = None;
                session.step(TradeEvent::SocketClosed, now)
            }
            Inbound::Frame(bytes) => match classify(&bytes) {
                Some(message) => session.step(TradeEvent::Inbound(message), now),
                None => session.step(TradeEvent::Unreadable, now),
            },
        };
        perform(actions, &mut session, &mut socket, &setup.url, &auth, &shared, base).await;
    }
    if let Some(mut ws) = socket.take() {
        ws.close().await;
    }
}

/// Carries out the session's actions, and those its answers lead to, in order.
async fn perform(
    actions: Vec<TradeAction>,
    session: &mut TradeSession,
    socket: &mut Option<WsStream>,
    url: &str,
    auth: &str,
    shared: &Shared,
    base: Instant,
) {
    let mut queue: VecDeque<TradeAction> = actions.into();
    while let Some(action) = queue.pop_front() {
        let follow = match action {
            TradeAction::Connect => {
                let event = match connect(url).await {
                    Some(ws) => {
                        *socket = Some(ws);
                        TradeEvent::Connected
                    }
                    None => TradeEvent::ConnectFailed,
                };
                session.step(event, elapsed_ns(base))
            }
            TradeAction::Authenticate => {
                send(socket, Outbound::Text(auth.to_string()), session, base).await
            }
            TradeAction::Send(frame) => send(socket, Outbound::Text(frame), session, base).await,
            TradeAction::Ping => send(socket, Outbound::Ping, session, base).await,
            TradeAction::Disconnect => {
                if let Some(mut ws) = socket.take() {
                    ws.close().await;
                }
                Vec::new()
            }
            TradeAction::Changed => {
                shared.changed.store(true, Ordering::Release);
                Vec::new()
            }
            TradeAction::Report(state) => {
                shared.live.store(state == TradeState::Live, Ordering::Release);
                Vec::new()
            }
        };
        queue.extend(follow);
    }
}

/// What the link sends: a text frame, or a ping whose pong shows the link is alive.
enum Outbound {
    Text(String),
    Ping,
}

/// A failed send is a closed socket.
async fn send(
    socket: &mut Option<WsStream>,
    message: Outbound,
    session: &mut TradeSession,
    base: Instant,
) -> Vec<TradeAction> {
    let Some(ws) = socket.as_mut() else { return Vec::new() };
    let sent = match message {
        Outbound::Text(text) => ws.send_text(text).await,
        Outbound::Ping => ws.send_ping().await,
    };
    if sent.is_ok() {
        return Vec::new();
    }
    *socket = None;
    session.step(TradeEvent::SocketClosed, elapsed_ns(base))
}

/// The handshake is bounded here: the session's own deadline cannot fire while the driver waits
/// inside it.
async fn connect(url: &str) -> Option<WsStream> {
    let bound = Duration::from_nanos(u64::try_from(ANSWER_TIMEOUT_NS).unwrap_or(u64::MAX));
    let limits = Limits { connect_timeout: bound, max_message_bytes: 1024 * 1024 };
    bijective_venue_socket::connect(url, limits).await.ok()
}

/// Waits forever without a socket. The caller's timer arm is the reconnect path.
async fn read_frame(socket: &mut Option<WsStream>) -> Option<Inbound> {
    match socket.as_mut() {
        None => std::future::pending().await,
        Some(ws) => match ws.next().await {
            Received::Text(text) => Some(Inbound::Frame(text.into_bytes())),
            Received::Binary(bytes) => Some(Inbound::Frame(bytes)),
            Received::Pong => Some(Inbound::Pong),
            Received::Closed => Some(Inbound::Closed),
        },
    }
}

/// Monotonic, so a clock correction cannot make the session think a deadline passed.
fn elapsed_ns(base: Instant) -> i128 {
    i128::try_from(base.elapsed().as_nanos()).unwrap_or(i128::MAX)
}
