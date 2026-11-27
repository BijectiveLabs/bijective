// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The socket only. Every decision lives in `session` so this stays thin and mechanical.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bijective_common::clock::Clock;
use bijective_venue::contract::StreamRequest;
use bijective_venue::profile::InstrumentScales;
use bijective_venue::resilience::SplitMix64;
use bijective_venue_socket::{Inbound, Limits, Socket, connect};
use tokio::sync::mpsc;

use crate::message::{ParseError, VenueMessage, parse};
use crate::session::{Action, ConnectionState, Event, Session, SessionError};

/// What the consumer above the socket sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// One frame as it left the socket, stamped on the platform clock before parsing, with what
    /// parsing made of it. An unreadable frame is carried, so venue assurance sees format changes.
    Frame {
        received_at_ns: i128,
        frame: String,
        parsed: Result<VenueMessage, ParseError>,
    },
    State(ConnectionState),
}

/// One frame exactly as sent, with its arrival time on the platform clock. The source for
/// recorded fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub at_ns: i128,
    pub frame: String,
}

/// Raised by whatever maintains the books. The socket cannot see a sequence gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    SequenceGap,
}

pub struct Connection {
    url: String,
    session: Session,
    scales: InstrumentScales,
    capture: Option<mpsc::Sender<Captured>>,
    clock: Arc<dyn Clock>,
    connect_timeout: Duration,
}

impl Connection {
    /// The topics are the venue's. The request carries the endpoint, scales and clock.
    pub fn new(request: &StreamRequest, topics: Vec<String>) -> Result<Self, SessionError> {
        let now = 0;
        Ok(Self {
            url: request.endpoint.clone(),
            session: Session::with_jitter(
                topics,
                now,
                Box::new(SplitMix64::for_this_process(request.clock.now_ns())),
            )?,
            scales: request.scales.clone(),
            capture: None,
            clock: Arc::clone(&request.clock),
            connect_timeout: Duration::from_nanos(
                u64::try_from(crate::session::CONNECT_TIMEOUT_NS).unwrap_or(u64::MAX),
            ),
        })
    }

    /// How long a handshake may take before it is given up on and reported. The session's own
    /// connect deadline cannot fire while the driver is waiting inside the handshake.
    #[must_use]
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// A dropped sink stops capture, never the connection.
    pub fn with_capture(mut self, sink: mpsc::Sender<Captured>) -> Self {
        self.capture = Some(sink);
        self
    }

    /// A closed `out` channel is the only shutdown signal. A second path would be a second way to fail to stop.
    pub async fn run(mut self, out: mpsc::Sender<Signal>, mut notices: mpsc::Receiver<Notice>) {
        let base = Instant::now();
        let mut socket: Option<WsStream> = None;

        loop {
            let now = elapsed_ns(base);

            if matches!(self.session.state(), ConnectionState::Failed(_)) {
                return;
            }

            let wake = self.session.wake_at();
            let sleep_for = wake
                .map(|w| Duration::from_nanos((w - now).max(0) as u64))
                .unwrap_or(Duration::from_secs(1));

            let event = tokio::select! {
                biased;

                notice = notices.recv() => match notice {
                    Some(Notice::SequenceGap) => Some(OwnedEvent::SequenceGap),
                    None => return,
                },

                frame = read_frame(&mut socket) => frame,

                _ = tokio::time::sleep(sleep_for) => Some(OwnedEvent::Tick),
            };

            let Some(event) = event else { continue };
            let now = elapsed_ns(base);
            let Some(actions) = self.on_event(event, &mut socket, &out, now).await else {
                return;
            };
            if !self.perform(actions, &mut socket, &out, base).await {
                return;
            }
        }
    }

    /// Steps the session on one event. `None` when the consumer has gone: a closed `out` channel
    /// is the shutdown signal.
    async fn on_event(
        &mut self,
        event: OwnedEvent,
        socket: &mut Option<WsStream>,
        out: &mpsc::Sender<Signal>,
        now: i128,
    ) -> Option<Vec<Action>> {
        Some(match event {
            OwnedEvent::Tick => self.session.step(Event::Tick, now),
            OwnedEvent::SequenceGap => self.session.step(Event::SequenceGap, now),
            OwnedEvent::Closed => {
                *socket = None;
                self.session.step(Event::SocketClosed, now)
            }
            OwnedEvent::Text(text) => {
                let received_at_ns = self.clock.now_ns();
                if let Some(sink) = self.capture.as_ref() {
                    let _ = sink.try_send(Captured { at_ns: received_at_ns, frame: text.clone() });
                }
                let parsed = parse(&text, &self.scales);
                let acts = match &parsed {
                    Ok(message) => self.session.step(Event::Inbound(message), now),
                    Err(_) => Vec::new(),
                };
                out.send(Signal::Frame { received_at_ns, frame: text, parsed }).await.ok()?;
                acts
            }
        })
    }

    /// Carries out the session's actions. False when the consumer has gone.
    async fn perform(
        &mut self,
        actions: Vec<Action>,
        socket: &mut Option<WsStream>,
        out: &mpsc::Sender<Signal>,
        base: Instant,
    ) -> bool {
        for action in actions {
            match action {
                Action::Connect => {
                    let ev = match connect(&self.url, socket_limits(self.connect_timeout)).await {
                        Ok(ws) => {
                            *socket = Some(ws);
                            Event::Connected
                        }
                        Err(_) => Event::ConnectFailed,
                    };
                    let follow = self.session.step(ev, elapsed_ns(base));
                    if !self.drain(follow, socket, out, base).await {
                        return false;
                    }
                }
                Action::Send(frame) => {
                    if let Some(ws) = socket.as_mut()
                        && ws.send_text(frame).await.is_err()
                    {
                        *socket = None;
                        let follow = self.session.step(Event::SocketClosed, elapsed_ns(base));
                        if !self.drain(follow, socket, out, base).await {
                            return false;
                        }
                    }
                }
                Action::Disconnect => {
                    if let Some(mut ws) = socket.take() {
                        ws.close().await;
                    }
                }
                // Books belong to the consumer. The state report carries this and is never implicit.
                Action::ResetBooks => {}
                Action::Report(state) => {
                    if out.send(Signal::State(state)).await.is_err() {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// One level deep suffices: a connect yields a subscribe, which yields nothing further.
    async fn drain(
        &mut self,
        actions: Vec<Action>,
        socket: &mut Option<WsStream>,
        out: &mpsc::Sender<Signal>,
        base: Instant,
    ) -> bool {
        for action in actions {
            match action {
                Action::Send(frame) => {
                    if let Some(ws) = socket.as_mut()
                        && ws.send_text(frame).await.is_err()
                    {
                        *socket = None;
                        let now = elapsed_ns(base);
                        let _ = self.session.step(Event::SocketClosed, now);
                    }
                }
                Action::Disconnect => {
                    if let Some(mut ws) = socket.take() {
                        ws.close().await;
                    }
                }
                Action::Report(state) => {
                    if out.send(Signal::State(state)).await.is_err() {
                        return false;
                    }
                }
                Action::Connect | Action::ResetBooks => {}
            }
        }
        true
    }
}

/// The largest message or frame taken. A book frame is kilobytes. The library default of 64 MiB
/// per message would let a peer hold that much memory.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The bounds every connection to the venue opens with.
pub(crate) fn socket_limits(connect_timeout: Duration) -> Limits {
    Limits { connect_timeout, max_message_bytes: MAX_MESSAGE_BYTES }
}

type WsStream = Socket;

enum OwnedEvent {
    Tick,
    SequenceGap,
    Closed,
    Text(String),
}

/// Waits forever without a socket. The caller select always has a timer arm, which is the reconnect path.
async fn read_frame(socket: &mut Option<WsStream>) -> Option<OwnedEvent> {
    match socket.as_mut() {
        None => std::future::pending().await,
        Some(ws) => match ws.next().await {
            Inbound::Text(text) => Some(OwnedEvent::Text(text)),
            Inbound::Binary(_) | Inbound::Pong => None,
            Inbound::Closed => Some(OwnedEvent::Closed),
        },
    }
}

/// Monotonic, so a clock correction cannot make the session think a deadline passed.
fn elapsed_ns(base: Instant) -> i128 {
    base.elapsed().as_nanos() as i128
}
