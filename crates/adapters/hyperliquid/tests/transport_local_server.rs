// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The socket driver against a local WebSocket server, which stands in for the venue so the
//! driver is exercised end to end: connect, subscribe, receive, report.
//!
//! Each test computes an outcome inside a runtime it then abandons and asserts afterwards. A driver
//! that never yields cannot be aborted, and a panic inside the runtime would wait for it forever.

use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_venue::contract::{ConnectionState, StreamRequest};
use bijective_venue::normalise::Scale;
use bijective_venue_hyperliquid::message::VenueMessage;
use bijective_venue_hyperliquid::transport::{Connection, Signal};
use futures_util::{SinkExt, StreamExt};
use sockudo_ws::server::WebSocketServer;
use sockudo_ws::{Config, Http1, Message};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

/// A test that waits longer than this has found a hang, which is reported as a failure.
const BOUND: Duration = Duration::from_secs(10);

const BOOK_FRAME: &str = r#"{"channel":"l2Book","data":{"coin":"BTC","time":1700000000000,"levels":[[{"px":"100.50","sz":"1.200","n":3}],[{"px":"100.60","sz":"0.800","n":2}]]}}"#;

/// Accepts one connection, reports the first frame the client sent, then answers with a book and
/// holds the connection open until the client goes away.
async fn serve_one_book(listener: TcpListener, first_frame: oneshot::Sender<String>) {
    let (stream, _) = listener.accept().await.expect("a client connects");
    let mut socket = accept(stream).await;
    let first = match socket.next().await {
        Some(Ok(Message::Text(text))) => String::from_utf8_lossy(&text).into_owned(),
        other => panic!("the first frame was not text: {other:?}"),
    };
    let _ = first_frame.send(first);
    socket.send(Message::Text(BOOK_FRAME.into())).await.expect("the book is sent");
    while socket.next().await.is_some() {}
}

/// Runs a future on its own runtime and abandons the runtime afterwards.
fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let output = runtime.block_on(future);
    runtime.shutdown_background();
    output
}

/// A client request for a local server that is already listening, and the channel that reports
/// the first frame the server heard.
async fn local_setup() -> (StreamRequest, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a local port");
    let request = StreamRequest {
        endpoint: format!("ws://{}", listener.local_addr().expect("an address")),
        instruments: vec!["BTC".to_string()],
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            ["BTC", "ETH"],
            bijective_venue::profile::Scales {
                price: Scale::new(2).expect("price scale"),
                quantity: Scale::new(3).expect("quantity scale"),
            },
        ),
        clock: Arc::new(WallClock::new()),
    };
    let (first_tx, first_rx) = oneshot::channel();
    tokio::spawn(serve_one_book(listener, first_tx));
    (request, first_rx)
}

/// Without a socket the driver must wait on its timer. It once returned from the read at once, so
/// the timer was never reached, no connection was ever attempted and the task spun.
#[test]
fn the_driver_connects_without_being_told_to() {
    let first_book = run(async {
        let (request, _first) = local_setup().await;
        let (tx, mut rx) = mpsc::channel::<Signal>(64);
        tokio::spawn(
            Connection::new(&request, request.scales.uniform().expect("one pair"))
                .expect("a session")
                .run(tx),
        );
        tokio::time::timeout(BOUND, async {
            while let Some(signal) = rx.recv().await {
                if let Signal::Frame { parsed: Ok(VenueMessage::Book { symbol, .. }), .. } = signal
                {
                    return Some(symbol);
                }
            }
            None
        })
        .await
    });

    match first_book {
        Ok(Some(symbol)) => assert_eq!(symbol, "BTC"),
        Ok(None) => panic!("the driver stopped without delivering a book"),
        Err(_) => panic!("no book arrived within {BOUND:?}: the driver never connected"),
    }
}

#[test]
fn the_first_frame_sent_subscribes_to_the_requested_coin() {
    let heard = run(async {
        let (request, first) = local_setup().await;
        let (tx, _rx) = mpsc::channel::<Signal>(64);
        tokio::spawn(
            Connection::new(&request, request.scales.uniform().expect("one pair"))
                .expect("a session")
                .run(tx),
        );
        tokio::time::timeout(BOUND, first).await
    });

    let frame = heard.expect("the server heard from the client within the bound").expect("a frame");
    let sent: serde_json::Value = serde_json::from_str(&frame).expect("the frame is json");
    assert_eq!(
        sent,
        serde_json::json!({
            "method": "subscribe",
            "subscription": { "type": "l2Book", "coin": "BTC" }
        })
    );
}

#[test]
fn a_connection_reports_the_states_it_passes_through() {
    let (reached, states) = run(async {
        let (request, _first) = local_setup().await;
        let (tx, mut rx) = mpsc::channel::<Signal>(64);
        tokio::spawn(
            Connection::new(&request, request.scales.uniform().expect("one pair"))
                .expect("a session")
                .run(tx),
        );
        let mut states = Vec::new();
        let reached = tokio::time::timeout(BOUND, async {
            while let Some(signal) = rx.recv().await {
                if let Signal::State(state) = signal {
                    let connected = matches!(state, ConnectionState::Connected { .. });
                    states.push(state);
                    if connected {
                        return true;
                    }
                }
            }
            false
        })
        .await;
        (reached, states)
    });

    assert!(reached == Ok(true), "never reported connected; states seen: {states:?}");
}

/// A server that accepts the connection and never completes the handshake. The driver must give
/// up after its connect timeout and report it. It once waited for the handshake forever, blocking
/// itself with no signal to the platform.
#[test]
fn a_handshake_that_never_completes_is_given_up_on_and_reported() {
    let states = run(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a local port");
        let request = StreamRequest {
            endpoint: format!("ws://{}", listener.local_addr().expect("an address")),
            instruments: vec!["BTC".to_string()],
            scales: bijective_venue::profile::InstrumentScales::uniform_over(
                ["BTC", "ETH"],
                bijective_venue::profile::Scales {
                    price: Scale::new(2).expect("price scale"),
                    quantity: Scale::new(3).expect("quantity scale"),
                },
            ),
            clock: Arc::new(WallClock::new()),
        };
        // Held open and silent for the whole test.
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let (tx, mut rx) = mpsc::channel::<Signal>(64);
        let connection = Connection::new(&request, request.scales.uniform().expect("one pair"))
            .expect("a session")
            .with_connect_timeout(Duration::from_millis(200));
        tokio::spawn(connection.run(tx));

        let mut states = Vec::new();
        let _ = tokio::time::timeout(BOUND, async {
            while let Some(signal) = rx.recv().await {
                if let Signal::State(state) = signal {
                    let given_up = matches!(state, ConnectionState::Disconnected);
                    states.push(state);
                    if given_up {
                        return;
                    }
                }
            }
        })
        .await;
        states
    });

    assert!(
        states.iter().any(|s| matches!(s, ConnectionState::Disconnected)),
        "the driver never gave up on the handshake; states seen: {states:?}"
    );
}

/// A peer that sends one frame past the bound. The driver drops the connection instead of holding
/// the frame, and nothing larger than the bound is ever delivered.
#[test]
fn a_frame_past_the_size_bound_is_not_delivered_and_the_connection_is_dropped() {
    let (largest, dropped) = run(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a local port");
        let request = StreamRequest {
            endpoint: format!("ws://{}", listener.local_addr().expect("an address")),
            instruments: vec!["BTC".to_string()],
            scales: bijective_venue::profile::InstrumentScales::uniform_over(
                ["BTC", "ETH"],
                bijective_venue::profile::Scales {
                    price: Scale::new(2).expect("price scale"),
                    quantity: Scale::new(3).expect("quantity scale"),
                },
            ),
            clock: Arc::new(WallClock::new()),
        };
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("a client connects");
            let mut socket = accept(stream).await;
            let _ = socket.next().await;
            let huge = "x".repeat(5 * 1024 * 1024);
            let _ = socket.send(Message::Text(huge.into())).await;
            while socket.next().await.is_some() {}
        });
        let (tx, mut rx) = mpsc::channel::<Signal>(64);
        tokio::spawn(
            Connection::new(&request, request.scales.uniform().expect("one pair"))
                .expect("a session")
                .run(tx),
        );

        let mut largest = 0usize;
        let dropped = tokio::time::timeout(BOUND, async {
            while let Some(signal) = rx.recv().await {
                match signal {
                    Signal::Frame { frame, .. } => largest = largest.max(frame.len()),
                    Signal::State(ConnectionState::Disconnected) => return true,
                    Signal::State(_) => {}
                }
            }
            false
        })
        .await;
        (largest, dropped)
    });

    assert_eq!(dropped, Ok(true), "the oversized frame did not end the connection");
    assert!(largest <= 4 * 1024 * 1024, "a frame of {largest} bytes was delivered");
}

/// The venue side of the link: a plain websocket server, which declines the client's deflate
/// offer, so the uncompressed path is the one these tests drive.
async fn accept(
    stream: tokio::net::TcpStream,
) -> sockudo_ws::WebSocketStream<sockudo_ws::Stream<Http1>> {
    let server = WebSocketServer::<Http1>::new(Config::default());
    server.accept(stream).await.expect("a handshake").0
}
