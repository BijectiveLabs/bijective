// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The socket driver against a local TCP server that stands in for a venue that does not answer.

use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_venue::contract::StreamRequest;
use bijective_venue::normalise::Scale;
use bijective_venue_bybit::session::ConnectionState;
use bijective_venue_bybit::transport::{Connection, Signal};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A test that waits longer than this has found a hang, which is reported as a failure.
const BOUND: Duration = Duration::from_secs(10);

/// A server that accepts the connection and never completes the handshake. The driver must give
/// up after its connect timeout and report it. It once waited for the handshake forever, and the
/// session's own connect deadline cannot fire while the driver is waiting inside it.
#[test]
fn a_handshake_that_never_completes_is_given_up_on_and_reported() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let states = runtime.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a local port");
        let request = StreamRequest {
            endpoint: format!("ws://{}", listener.local_addr().expect("an address")),
            instruments: vec!["BTCUSDT".to_string()],
            scales: bijective_venue::profile::InstrumentScales::uniform_over(
                ["BTCUSDT", "ETHUSDT"],
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
        let (_notices, notice_rx) = mpsc::channel(4);
        let connection = Connection::new(&request, vec!["orderbook.50.BTCUSDT".to_string()])
            .expect("a session")
            .with_connect_timeout(Duration::from_millis(200));
        tokio::spawn(connection.run(tx, notice_rx));

        let mut states = Vec::new();
        let _ = tokio::time::timeout(BOUND, async {
            while let Some(signal) = rx.recv().await {
                if let Signal::State(state) = signal {
                    let given_up = matches!(state, ConnectionState::Degraded(_));
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
    runtime.shutdown_background();

    assert!(
        states.iter().any(|s| matches!(s, ConnectionState::Degraded(_))),
        "the driver never gave up on the handshake; states seen: {states:?}"
    );
}
