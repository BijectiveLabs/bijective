// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The socket and nothing else. Every decision belongs to `session`.
//! Thinnest part of the adapter, because it cannot be tested without a venue.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bijective_common::clock::Clock;
use bijective_venue::contract::{ConnectionState, StreamRequest};
use bijective_venue::normalise::Scale;
use bijective_venue::resilience::SplitMix64;
use bijective_venue_socket::{Inbound as Received, Limits, Socket};
use tokio::sync::mpsc;

use crate::message::{ParseError, VenueEvent, parse};
use crate::session::{Action, Event, Session};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// One frame as it left the socket, stamped on the platform clock before parsing, with what
    /// parsing made of it. Surfaced either way: a skipped frame hides a format change.
    Frame {
        received_at_ns: i128,
        frame: String,
        parsed: Result<Vec<VenueEvent>, ParseError>,
    },
    State(ConnectionState),
}

/// How long a handshake may take by default.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest message or frame taken. The opening snapshot of many outcomes is hundreds of
/// kilobytes. The library default of 64 MiB per message would let a peer hold that much memory.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The bounds every connection to the venue opens with.
fn socket_limits(connect_timeout: std::time::Duration) -> Limits {
    Limits { connect_timeout, max_message_bytes: MAX_MESSAGE_BYTES }
}

type WsStream = Socket;

enum Arrival {
    Tick,
    Closed,
    Text(String),
    Resnapshot(String),
}

pub struct Connection {
    connect_timeout: Duration,
    url: String,
    session: Session,
    price: Scale,
    quantity: Scale,
    clock: Arc<dyn Clock>,
}

impl Connection {
    /// No sequence-gap channel: this venue numbers nothing, so none could be raised.
    /// `scales` is the one pair this venue's instruments share (decision 0007).
    pub fn new(
        request: &StreamRequest,
        scales: bijective_venue::profile::Scales,
    ) -> Result<Self, crate::session::SessionError> {
        Ok(Self {
            connect_timeout: CONNECT_TIMEOUT,
            url: request.endpoint.clone(),
            session: Session::with_jitter(
                request.instruments.clone(),
                0,
                Box::new(SplitMix64::for_this_process(request.clock.now_ns())),
            )?,
            price: scales.price,
            quantity: scales.quantity,
            clock: Arc::clone(&request.clock),
        })
    }

    /// A closed `out` channel is the only shutdown signal. A second way to stop is a second way
    /// to fail to stop.
    /// `resnapshot` carries outcomes whose book the feed refused. Each is asked for again.
    pub async fn run(mut self, out: mpsc::Sender<Signal>, mut resnapshot: mpsc::Receiver<String>) {
        let base = Instant::now();
        let mut socket: Option<WsStream> = None;

        loop {
            let now = elapsed_ns(base);
            let sleep_for = self.session.wake_at().map_or(Duration::from_secs(1), |w| {
                Duration::from_nanos(u64::try_from((w - now).max(0)).unwrap_or(u64::MAX))
            });

            let arrival = tokio::select! {
                biased;
                Some(instrument) = resnapshot.recv() => Some(Arrival::Resnapshot(instrument)),
                frame = read_frame(&mut socket) => frame,
                () = tokio::time::sleep(sleep_for) => Some(Arrival::Tick),
            };

            let Some(arrival) = arrival else { continue };
            let now = elapsed_ns(base);
            // Taken before parsing, so the stamp measures the venue and the network, not us.
            let received_at_ns = self.clock.now_ns();

            let actions = match arrival {
                Arrival::Tick => self.session.step(Event::Tick, now),
                Arrival::Resnapshot(instrument) => {
                    self.session.step(Event::Resnapshot(&instrument), now)
                }
                Arrival::Closed => {
                    socket = None;
                    self.session.step(Event::SocketClosed, now)
                }
                Arrival::Text(text) => {
                    let parsed = parse(text.as_bytes(), self.price, self.quantity);
                    let acts = match &parsed {
                        Ok(events) => self.session.step(Event::Inbound(events), now),
                        Err(_) => Vec::new(),
                    };
                    let signal = Signal::Frame { received_at_ns, frame: text, parsed };
                    if out.send(signal).await.is_err() {
                        return;
                    }
                    acts
                }
            };

            if !self.carry_out(actions, &mut socket, &out, base).await {
                return;
            }
        }
    }

    async fn carry_out(
        &mut self,
        actions: Vec<Action>,
        socket: &mut Option<WsStream>,
        out: &mpsc::Sender<Signal>,
        base: Instant,
    ) -> bool {
        let mut queue = actions;
        while let Some(action) = queue.first().cloned() {
            queue.remove(0);

            match action {
                Action::Report(state) => {
                    if out.send(Signal::State(state)).await.is_err() {
                        return false;
                    }
                }
                Action::Connect => {
                    let attempt = bijective_venue_socket::connect(
                        &self.url,
                        socket_limits(self.connect_timeout),
                    )
                    .await;
                    let event = match attempt {
                        Ok(ws) => {
                            *socket = Some(ws);
                            Event::Connected
                        }
                        Err(_) => Event::ConnectFailed,
                    };
                    let follow = self.session.step(event, elapsed_ns(base));
                    queue.extend(follow);
                }
                Action::Send(frame) => {
                    let sent = match socket.as_mut() {
                        Some(ws) => ws.send_text(frame).await.is_ok(),
                        None => false,
                    };
                    if !sent {
                        *socket = None;
                        let follow = self.session.step(Event::SocketClosed, elapsed_ns(base));
                        queue.extend(follow);
                    }
                }
                Action::Disconnect => {
                    if let Some(mut ws) = socket.take() {
                        ws.close().await;
                    }
                }
                // The books belong to the feed above this, which is told by the state it reads.
                Action::ResetBooks => {}
            }
        }
        true
    }
}

/// Waits forever without a socket: the caller's select always has a timer arm, and that timer is
/// the only way a connection is first made and later remade. Returning here would starve it.
async fn read_frame(socket: &mut Option<WsStream>) -> Option<Arrival> {
    let Some(ws) = socket.as_mut() else {
        return std::future::pending().await;
    };
    match ws.next().await {
        Received::Text(text) => Some(Arrival::Text(text)),
        Received::Binary(_) | Received::Pong => None,
        Received::Closed => Some(Arrival::Closed),
    }
}

fn elapsed_ns(base: Instant) -> i128 {
    i128::try_from(base.elapsed().as_nanos()).unwrap_or(i128::MAX)
}
