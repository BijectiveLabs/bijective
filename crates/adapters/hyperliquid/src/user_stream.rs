// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The account stream's socket, run on a thread of its own so the node's pass cadence never starves
//! its heartbeat. It holds no order state: it raises a flag when the venue reports a change, and
//! the venue evidence read stays the one source of order states and fills.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bijective_common::clock::Clock;
use bijective_venue::contract::Pushed;
use bijective_venue::resilience::SplitMix64;
use bijective_venue_socket::{Inbound as Received, Socket};

use crate::transport::socket_limits;
use crate::user_session::{
    ANSWER_TIMEOUT_NS, UserAction, UserEvent, UserSession, UserState, classify,
};

/// The longest the driver waits before it looks at the stop flag again.
const STOP_POLL: Duration = Duration::from_secs(1);

/// The stream for a REST endpoint, on the host the venue pairs with it (WebSocket page, read
/// 2026-11-16). Any other endpoint has none this build knows, and its venue is read on the evidence
/// cadence alone.
#[must_use]
pub fn endpoint_for(rest_endpoint: &str) -> Option<&'static str> {
    match rest_endpoint.trim_end_matches('/') {
        "https://api.hyperliquid.xyz" => Some("wss://api.hyperliquid.xyz/ws"),
        "https://api.hyperliquid-testnet.xyz" => Some("wss://api.hyperliquid-testnet.xyz/ws"),
        _ => None,
    }
}

/// What the link needs: the account's address, never the signer's (an agent wallet's address
/// reads empty, Subscriptions page).
pub struct LinkSetup {
    pub url: String,
    pub user: String,
    /// Seeds the reconnect jitter.
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
pub struct UserLink {
    shared: Arc<Shared>,
}

impl UserLink {
    /// Starts the link's thread. An `Err` is the operating system refusing a thread.
    pub fn start(setup: LinkSetup) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let held = Arc::clone(&shared);
        std::thread::Builder::new().name("hyperliquid-user".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build()
            else {
                return;
            };
            runtime.block_on(run(setup, held));
        })?;
        Ok(Self { shared })
    }

    /// What the link said since the last ask. A report outranks the link's state: a change pushed
    /// just before the link lapsed is still a change.
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

impl Drop for UserLink {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

type WsStream = Socket;

enum Inbound {
    Tick,
    Closed,
    Text(String),
}

async fn run(setup: LinkSetup, shared: Arc<Shared>) {
    let base = Instant::now();
    let jitter = Box::new(SplitMix64::for_this_process(setup.clock.now_ns()));
    let mut session = UserSession::new(setup.user.clone(), 0, jitter);
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
            Inbound::Tick => session.step(UserEvent::Tick, now),
            Inbound::Closed => {
                socket = None;
                session.step(UserEvent::SocketClosed, now)
            }
            Inbound::Text(text) => match classify(&text) {
                Some(message) => session.step(UserEvent::Inbound(message), now),
                None => session.step(UserEvent::Unreadable, now),
            },
        };
        perform(actions, &mut session, &mut socket, &setup, &shared, base).await;
    }
    if let Some(mut ws) = socket.take() {
        ws.close().await;
    }
}

/// Carries out the session's actions, and those its answers lead to, in order.
async fn perform(
    actions: Vec<UserAction>,
    session: &mut UserSession,
    socket: &mut Option<WsStream>,
    setup: &LinkSetup,
    shared: &Shared,
    base: Instant,
) {
    let mut queue: VecDeque<UserAction> = actions.into();
    while let Some(action) = queue.pop_front() {
        let follow = match action {
            UserAction::Connect => {
                let event = match connect(&setup.url).await {
                    Some(ws) => {
                        *socket = Some(ws);
                        UserEvent::Connected
                    }
                    None => UserEvent::ConnectFailed,
                };
                session.step(event, elapsed_ns(base))
            }
            UserAction::Send(frame) => send(socket, frame, session, base).await,
            UserAction::Disconnect => {
                if let Some(mut ws) = socket.take() {
                    ws.close().await;
                }
                Vec::new()
            }
            UserAction::Changed => {
                shared.changed.store(true, Ordering::Release);
                Vec::new()
            }
            UserAction::Report(state) => {
                shared.live.store(state == UserState::Live, Ordering::Release);
                Vec::new()
            }
        };
        queue.extend(follow);
    }
}

/// A failed send is a closed socket.
async fn send(
    socket: &mut Option<WsStream>,
    frame: String,
    session: &mut UserSession,
    base: Instant,
) -> Vec<UserAction> {
    let Some(ws) = socket.as_mut() else { return Vec::new() };
    if ws.send_text(frame).await.is_ok() {
        return Vec::new();
    }
    *socket = None;
    session.step(UserEvent::SocketClosed, elapsed_ns(base))
}

/// The handshake is bounded here: the session's own connect deadline cannot fire while the driver
/// waits inside it.
async fn connect(url: &str) -> Option<WsStream> {
    let bound = Duration::from_nanos(u64::try_from(ANSWER_TIMEOUT_NS).unwrap_or(u64::MAX));
    bijective_venue_socket::connect(url, socket_limits(bound)).await.ok()
}

/// Waits forever without a socket. The caller's timer arm is the reconnect path.
async fn read_frame(socket: &mut Option<WsStream>) -> Option<Inbound> {
    match socket.as_mut() {
        None => std::future::pending().await,
        Some(ws) => match ws.next().await {
            Received::Text(text) => Some(Inbound::Text(text)),
            Received::Binary(_) | Received::Pong => None,
            Received::Closed => Some(Inbound::Closed),
        },
    }
}

/// Monotonic, so a clock correction cannot make the session think a deadline passed.
fn elapsed_ns(base: Instant) -> i128 {
    i128::try_from(base.elapsed().as_nanos()).unwrap_or(i128::MAX)
}
