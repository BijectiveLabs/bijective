// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The private stream's driver against a local server that answers as the venue documents
//! (WebSocket connect guide, Order stream): it authenticates, subscribes, reports a change, and
//! lapses when the server goes away.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bijective_common::clock::WallClock;
use bijective_secrets_broker::credential::Credential;
use bijective_venue::contract::Pushed;
use bijective_venue::normalise::Scale;
use bijective_venue_bybit::private_stream::{LinkSetup, PrivateLink, endpoint_for};
use futures_util::{SinkExt, StreamExt};
use sockudo_ws::server::WebSocketServer;
use sockudo_ws::{Config, Http1, Message};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A test that waits longer than this has found a hang, which is reported as a failure.
const BOUND: Duration = Duration::from_secs(10);

const ORDER_FRAME: &str =
    r#"{"topic":"order.linear","id":"1","creationTime":1700000000000,"data":[]}"#;

/// Polls the link until `wanted` or the bound. The last answer is returned either way.
fn wait_for(link: &PrivateLink, wanted: Pushed) -> Pushed {
    let start = Instant::now();
    loop {
        let pushed = link.pushed();
        if pushed == wanted || start.elapsed() > BOUND {
            return pushed;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_link_authenticates_subscribes_reports_and_lapses() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).expect("a local port");
    let url = format!("ws://{}", listener.local_addr().expect("an address"));
    let (report, reported) = oneshot::channel::<()>();
    let (close, closed) = oneshot::channel::<()>();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
    runtime.spawn(async move {
        let (stream, _) = listener.accept().await.expect("a connection");
        let mut ws = accept(stream).await;
        for answer in [
            r#"{"success":true,"ret_msg":"","op":"auth","conn_id":"c1"}"#,
            r#"{"success":true,"ret_msg":"","op":"subscribe","conn_id":"c1"}"#,
        ] {
            let Some(Ok(Message::Text(asked))) = ws.next().await else { return };
            let _ = seen_tx.send(String::from_utf8_lossy(&asked).into_owned());
            ws.send(Message::Text(answer.into())).await.expect("an answer");
        }
        let _ = reported.await;
        ws.send(Message::Text(ORDER_FRAME.into())).await.expect("a report");
        let _ = closed.await;
        let _ = SinkExt::close(&mut ws).await;
    });

    let link = PrivateLink::start(LinkSetup {
        url,
        api_key: "key".into(),
        secret: Credential::new(b"secret".to_vec()),
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            ["BTCUSDT", "ETHUSDT"],
            bijective_venue::profile::Scales {
                price: Scale::new(2).expect("price scale"),
                quantity: Scale::new(3).expect("quantity scale"),
            },
        ),
        clock: Arc::new(WallClock::new()),
    })
    .expect("a thread");

    assert_eq!(wait_for(&link, Pushed::Changed), Pushed::Changed, "attaching asks for a read");
    assert_eq!(link.pushed(), Pushed::Quiet, "asking clears the report");
    let auth: serde_json::Value =
        serde_json::from_str(&seen_rx.recv_timeout(BOUND).expect("an auth frame")).expect("JSON");
    assert_eq!(auth["op"], "auth");
    assert_eq!(auth["args"][0], "key");
    let subscribe = seen_rx.recv_timeout(BOUND).expect("a subscribe frame");
    assert!(subscribe.contains("order.linear") && subscribe.contains("execution.linear"));

    report.send(()).expect("the server waits");
    assert_eq!(
        wait_for(&link, Pushed::Changed),
        Pushed::Changed,
        "an order report asks for a read"
    );
    close.send(()).expect("the server waits");
    assert_eq!(wait_for(&link, Pushed::Lapsed), Pushed::Lapsed, "a closed link lapses");
    drop(link);
    runtime.shutdown_background();
}

#[test]
fn only_the_documented_hosts_have_a_private_stream() {
    assert_eq!(endpoint_for("https://api.bybit.com/"), Some("wss://stream.bybit.com/v5/private"));
    assert_eq!(
        endpoint_for("https://api-testnet.bybit.com"),
        Some("wss://stream-testnet.bybit.com/v5/private")
    );
    assert_eq!(
        endpoint_for("https://api-demo.bybit.com"),
        Some("wss://stream-demo.bybit.com/v5/private")
    );
    assert_eq!(endpoint_for("https://api.bytick.com"), None);
    assert_eq!(endpoint_for("http://127.0.0.1:9"), None);
}

/// The venue side of the link: a plain websocket server, which declines the client's deflate
/// offer, so the uncompressed path is the one these tests drive.
async fn accept(
    stream: tokio::net::TcpStream,
) -> sockudo_ws::WebSocketStream<sockudo_ws::Stream<Http1>> {
    let server = WebSocketServer::<Http1>::new(Config::default());
    server.accept(stream).await.expect("a handshake").0
}
