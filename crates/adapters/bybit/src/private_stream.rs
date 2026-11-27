// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The private stream's socket, run on a thread of its own so the node's pass cadence never starves
//! its heartbeat. It raises a flag when the venue reports a change and keeps the fills it pushed,
//! which the next evidence read adds where the venue's history does not list them yet. Order
//! states still come from the read alone.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bijective_lifecycle::FillReport;
use std::time::{Duration, Instant};

use bijective_common::clock::Clock;
use bijective_secrets_broker::credential::Credential;
use bijective_venue::contract::Pushed;
use bijective_venue::resilience::SplitMix64;
use bijective_venue_socket::{Inbound as Received, Socket};

use crate::evidence::InstrumentScales;
use crate::private::{PrivateMessage, auth_frame, parse, retain_pushed};
use crate::private_session::{PrivateAction, PrivateEvent, PrivateSession, PrivateState};
use crate::session::CONNECT_TIMEOUT_NS;
use crate::transport::socket_limits;

/// How long the authentication frame stays valid after it is signed. The venue refuses an expiry
/// already past when the frame arrives (WebSocket connect guide, Authentication).
const AUTH_VALID_FOR_MS: u64 = 10_000;

/// The longest the driver waits before it looks at the stop flag again.
const STOP_POLL: Duration = Duration::from_secs(1);

/// The private stream for a REST endpoint, on the host the venue pairs with it (WebSocket connect
/// guide, read 2026-11-16, and Demo trading service, read 2026-11-17, which names `stream-demo` for
/// private streams only and the `/v5/private` path). Any other endpoint has none this build knows,
/// and its venue is read on the evidence cadence alone. A regional host is refused, not guessed.
#[must_use]
pub fn endpoint_for(rest_endpoint: &str) -> Option<&'static str> {
    match rest_endpoint.trim_end_matches('/') {
        "https://api.bybit.com" => Some("wss://stream.bybit.com/v5/private"),
        "https://api-testnet.bybit.com" => Some("wss://stream-testnet.bybit.com/v5/private"),
        "https://api-demo.bybit.com" => Some("wss://stream-demo.bybit.com/v5/private"),
        _ => None,
    }
}

/// What the link needs. The secret signs the authentication frame and nothing else.
pub struct LinkSetup {
    pub url: String,
    pub api_key: String,
    pub secret: Credential,
    pub scales: InstrumentScales,
    /// The wall clock the authentication expiry is written on.
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, Default)]
struct Shared {
    changed: AtomicBool,
    live: AtomicBool,
    stop: AtomicBool,
    fills: Mutex<Vec<FillReport>>,
}

/// The running link. Dropping it stops the thread at its next look at the stop flag.
#[derive(Debug)]
pub struct PrivateLink {
    shared: Arc<Shared>,
}

impl PrivateLink {
    /// Starts the link's thread. An `Err` is the operating system refusing a thread.
    pub fn start(setup: LinkSetup) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let held = Arc::clone(&shared);
        std::thread::Builder::new().name("bybit-private".into()).spawn(move || {
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

    /// The fills the venue pushed within the retention bound, oldest first. A holder that
    /// panicked left whole rows behind, so its lock is taken over rather than refused.
    pub fn pushed_fills(&self) -> Vec<FillReport> {
        self.shared.fills.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Drop for PrivateLink {
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
    let mut session = PrivateSession::new(0, jitter);
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
            Inbound::Tick => session.step(PrivateEvent::Tick, now),
            Inbound::Closed => {
                socket = None;
                session.step(PrivateEvent::SocketClosed, now)
            }
            Inbound::Text(text) => match parse(&text, &setup.scales, setup.clock.now_ns()) {
                Ok(message) => {
                    // Kept before the change is flagged, so the read it prompts finds the fill.
                    if let PrivateMessage::Executions(rows) = &message {
                        let mut held = shared.fills.lock().unwrap_or_else(PoisonError::into_inner);
                        retain_pushed(&mut held, rows, setup.clock.now_ns());
                    }
                    session.step(PrivateEvent::Inbound(&message), now)
                }
                Err(_) => session.step(PrivateEvent::Unreadable, now),
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
    actions: Vec<PrivateAction>,
    session: &mut PrivateSession,
    socket: &mut Option<WsStream>,
    setup: &LinkSetup,
    shared: &Shared,
    base: Instant,
) {
    let mut queue: VecDeque<PrivateAction> = actions.into();
    while let Some(action) = queue.pop_front() {
        let follow = match action {
            PrivateAction::Connect => {
                let event = match connect(&setup.url).await {
                    Some(ws) => {
                        *socket = Some(ws);
                        PrivateEvent::Connected
                    }
                    None => PrivateEvent::ConnectFailed,
                };
                session.step(event, elapsed_ns(base))
            }
            PrivateAction::Authenticate => {
                let expires_ms = wall_ms(setup.clock.as_ref()) + AUTH_VALID_FOR_MS;
                let frame = auth_frame(&setup.api_key, setup.secret.expose(), expires_ms);
                send(socket, frame, session, base).await
            }
            PrivateAction::Send(frame) => send(socket, frame, session, base).await,
            PrivateAction::Disconnect => {
                if let Some(mut ws) = socket.take() {
                    ws.close().await;
                }
                Vec::new()
            }
            PrivateAction::Changed => {
                shared.changed.store(true, Ordering::Release);
                Vec::new()
            }
            PrivateAction::Report(state) => {
                shared.live.store(state == PrivateState::Live, Ordering::Release);
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
    session: &mut PrivateSession,
    base: Instant,
) -> Vec<PrivateAction> {
    let Some(ws) = socket.as_mut() else { return Vec::new() };
    if ws.send_text(frame).await.is_ok() {
        return Vec::new();
    }
    *socket = None;
    session.step(PrivateEvent::SocketClosed, elapsed_ns(base))
}

/// The handshake is bounded here: the session's own connect deadline cannot fire while the driver
/// waits inside it.
async fn connect(url: &str) -> Option<WsStream> {
    let bound = Duration::from_nanos(u64::try_from(CONNECT_TIMEOUT_NS).unwrap_or(u64::MAX));
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

fn wall_ms(clock: &dyn Clock) -> u64 {
    u64::try_from(clock.now_ns() / 1_000_000).unwrap_or(0)
}

/// Monotonic, so a clock correction cannot make the session think a deadline passed.
fn elapsed_ns(base: Instant) -> i128 {
    i128::try_from(base.elapsed().as_nanos()).unwrap_or(i128::MAX)
}
