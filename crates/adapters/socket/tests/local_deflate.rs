// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The deflate path end to end against a local venue that accepts the offer: the client reads the
//! venue compressed, writes to it compressed, and sees the end of the connection.

use std::time::Duration;

use bijective_venue_socket::{Inbound, Limits, connect};
use futures_util::{SinkExt, StreamExt};
use sockudo_ws::{CompressedWebSocketStream, Config, DeflateConfig, Message, handshake};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const LIMITS: Limits =
    Limits { connect_timeout: Duration::from_secs(5), max_message_bytes: 1 << 20 };

/// Reads the upgrade request, accepts deflate, and answers with a compressed stream.
async fn venue_that_compresses(
    listener: TcpListener,
) -> CompressedWebSocketStream<tokio::net::TcpStream> {
    let (mut stream, _) = listener.accept().await.expect("a client");
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.expect("the request");
        request.push(byte[0]);
    }
    let text = String::from_utf8(request).expect("text");
    assert!(text.contains("permessage-deflate"), "the client offers deflate: {text}");
    let key = text
        .lines()
        .find_map(|l| {
            l.strip_prefix("Sec-WebSocket-Key: ").or_else(|| l.strip_prefix("sec-websocket-key: "))
        })
        .expect("a key")
        .trim()
        .to_string();
    let answer = handshake::build_response(
        &handshake::generate_accept_key(&key),
        None,
        Some("permessage-deflate"),
    );
    stream.write_all(&answer).await.expect("the answer");
    CompressedWebSocketStream::server(stream, Config::default(), DeflateConfig::default())
}

#[tokio::test]
async fn a_venue_that_accepts_deflate_is_read_and_written_compressed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("ws://{}/stream", listener.local_addr().expect("an address"));
    let book = "{\"topic\":\"book\",\"data\":".to_string() + &"[1,2,3,4,5],".repeat(200) + "0}";
    let sent = book.clone();
    let venue = tokio::spawn(async move {
        let mut ws = venue_that_compresses(listener).await;
        let Some(Ok(Message::Text(asked))) = ws.next().await else { panic!("a subscription") };
        assert_eq!(&asked[..], b"{\"op\":\"subscribe\"}");
        ws.send(Message::Text(sent.into())).await.expect("a book");
        let _ = SinkExt::close(&mut ws).await;
    });
    let mut socket = connect(&url, LIMITS).await.expect("connected");
    assert!(socket.is_compressed(), "the venue accepted deflate");
    socket.send_text("{\"op\":\"subscribe\"}".into()).await.expect("sent");
    assert_eq!(socket.next().await, Inbound::Text(book));
    assert_eq!(socket.next().await, Inbound::Closed);
    venue.await.expect("the venue ran");
}

#[tokio::test]
async fn an_address_with_nothing_listening_fails_within_the_bound() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    drop(listener);
    assert!(connect(&url, LIMITS).await.is_err());
}
