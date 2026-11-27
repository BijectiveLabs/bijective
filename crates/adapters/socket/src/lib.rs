// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The websocket every venue adapter connects through. TLS is verified against the host's roots,
//! and the client offers permessage-deflate (RFC 7692): a venue that accepts it sends a fraction
//! of the bytes, so a narrow link does not fall behind the venue (finding 2026-11-24). A venue that
//! declines is read uncompressed through the same type. A ping from the venue is answered with a
//! pong by the library. Text, binary, a pong to the adapter's own ping and the end of the
//! connection reach the adapter.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use sockudo_ws::deflate::parse_deflate_offer;
use sockudo_ws::{
    CompressedWebSocketStream, Config, DeflateConfig, Message, Role, WebSocketStream, handshake,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// The client's offer: deflate, the venue free to choose its window and the client's.
const DEFLATE_OFFER: &str = "permessage-deflate; client_max_window_bits";

/// A handshake answer larger than this is not one.
const MAX_ANSWER_BYTES: usize = 16 * 1024;

/// The byte stream under the websocket: TLS for `wss`, plain TCP for `ws`.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
type Transport = Box<dyn Io>;

/// The bounds of one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// From the first TCP packet to the end of the upgrade.
    pub connect_timeout: Duration,
    /// The largest message, and frame, the venue may send.
    pub max_message_bytes: usize,
}

/// Why no connection was made, or a send failed. Each carries the cause as text for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketError {
    Address(String),
    Connect(String),
    Tls(String),
    Handshake(String),
    Send(String),
}

impl core::fmt::Display for SocketError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Address(why) => write!(f, "the address cannot be used: {why}"),
            Self::Connect(why) => write!(f, "no connection: {why}"),
            Self::Tls(why) => write!(f, "TLS failed: {why}"),
            Self::Handshake(why) => write!(f, "the websocket upgrade failed: {why}"),
            Self::Send(why) => write!(f, "a message could not be sent: {why}"),
        }
    }
}

impl std::error::Error for SocketError {}

/// What the venue sent, or the end of the connection, whatever ended it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    Text(String),
    Binary(Vec<u8>),
    /// The answer to a ping this client sent.
    Pong,
    Closed,
}

enum Stream {
    Plain(Box<WebSocketStream<Transport>>),
    Compressed(Box<CompressedWebSocketStream<Transport>>),
}

/// One open connection.
pub struct Socket {
    stream: Stream,
}

impl Socket {
    /// Whether the venue accepted permessage-deflate.
    #[must_use]
    pub fn is_compressed(&self) -> bool {
        matches!(self.stream, Stream::Compressed(_))
    }

    /// Sends `text` as one text message.
    ///
    /// # Errors
    /// `SocketError::Send` when the connection can no longer carry it.
    pub async fn send_text(&mut self, text: String) -> Result<(), SocketError> {
        let sent = match &mut self.stream {
            Stream::Plain(ws) => ws.send(Message::Text(Bytes::from(text))).await,
            Stream::Compressed(ws) => ws.send(Message::Text(Bytes::from(text))).await,
        };
        sent.map_err(|e| SocketError::Send(e.to_string()))
    }

    /// The next message. A ping is answered and not returned. A close, an error or the end of the
    /// stream are all `Closed`: the venue's state is then unknown and the session rebuilds it.
    pub async fn next(&mut self) -> Inbound {
        loop {
            let read = match &mut self.stream {
                Stream::Plain(ws) => ws.next().await,
                Stream::Compressed(ws) => ws.next().await,
            };
            match read {
                Some(Ok(Message::Text(bytes))) => {
                    return String::from_utf8(bytes.to_vec())
                        .map_or(Inbound::Closed, Inbound::Text);
                }
                Some(Ok(Message::Binary(bytes))) => return Inbound::Binary(bytes.to_vec()),
                Some(Ok(Message::Pong(_))) => return Inbound::Pong,
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Inbound::Closed,
            }
        }
    }

    /// Sends a ping, for a venue whose liveness the adapter checks by the pong.
    ///
    /// # Errors
    /// `SocketError::Send` when the connection can no longer carry it.
    pub async fn send_ping(&mut self) -> Result<(), SocketError> {
        let sent = match &mut self.stream {
            Stream::Plain(ws) => ws.send(Message::Ping(Bytes::new())).await,
            Stream::Compressed(ws) => ws.send(Message::Ping(Bytes::new())).await,
        };
        sent.map_err(|e| SocketError::Send(e.to_string()))
    }

    /// Sends a normal close. A connection that already failed has nothing to close.
    pub async fn close(&mut self) {
        let _ = match &mut self.stream {
            Stream::Plain(ws) => SinkExt::close(ws.as_mut()).await,
            Stream::Compressed(ws) => SinkExt::close(ws.as_mut()).await,
        };
    }
}

/// Opens `url` (`wss://` or `ws://`) within `limits.connect_timeout`.
///
/// # Errors
/// The `SocketError` that names the step that failed.
pub async fn connect(url: &str, limits: Limits) -> Result<Socket, SocketError> {
    tokio::time::timeout(limits.connect_timeout, open(url, limits))
        .await
        .map_err(|_| SocketError::Connect("timed out".into()))?
}

async fn open(url: &str, limits: Limits) -> Result<Socket, SocketError> {
    let target = Target::parse(url)?;
    let tcp = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|e| SocketError::Connect(e.to_string()))?;
    // A venue frame waits on no coalescing timer.
    let _ = tcp.set_nodelay(true);
    let mut io: Transport =
        if target.secure { Box::new(tls(tcp, &target.host).await?) } else { Box::new(tcp) };
    let (extensions, leftover) = upgrade(&mut io, &target).await?;
    let config = Config::builder()
        .max_message_size(limits.max_message_bytes)
        .max_frame_size(limits.max_message_bytes)
        .validate_text_utf8(true)
        .build();
    let stream = match accepted_deflate(extensions.as_deref())? {
        Some(deflate) => Stream::Compressed(Box::new(
            CompressedWebSocketStream::client_with_leftover(io, config, deflate, leftover),
        )),
        None => Stream::Plain(Box::new(WebSocketStream::from_raw_with_leftover(
            io,
            Role::Client,
            config,
            leftover,
        ))),
    };
    Ok(Socket { stream })
}

/// Where to connect, from a `ws` or `wss` address.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    secure: bool,
    host: String,
    port: u16,
    /// The `Host` header: the host, and the port when it is not the scheme's own.
    authority: String,
    /// Path and query.
    resource: String,
}

impl Target {
    fn parse(url: &str) -> Result<Self, SocketError> {
        let parsed = url::Url::parse(url).map_err(|e| SocketError::Address(e.to_string()))?;
        let secure = match parsed.scheme() {
            "wss" => true,
            "ws" => false,
            other => return Err(SocketError::Address(format!("{other} is not ws or wss"))),
        };
        let host = parsed.host_str().ok_or_else(|| SocketError::Address("no host".into()))?;
        let port = parsed.port_or_known_default().unwrap_or(if secure { 443 } else { 80 });
        let authority = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let resource = match parsed.query() {
            Some(query) => format!("{}?{query}", parsed.path()),
            None => parsed.path().to_string(),
        };
        Ok(Self { secure, host: host.to_string(), port, authority, resource })
    }
}

/// The TLS configuration, built once: the host's roots and the ring provider every adapter uses.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, SocketError> {
    static CONFIG: OnceLock<Result<Arc<rustls::ClientConfig>, SocketError>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(cert);
            }
            if roots.is_empty() {
                return Err(SocketError::Tls("this host holds no root certificate".into()));
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| SocketError::Tls(e.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
            Ok(Arc::new(config))
        })
        .clone()
}

async fn tls(
    tcp: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, SocketError> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| SocketError::Tls(e.to_string()))?;
    tokio_rustls::TlsConnector::from(tls_config()?)
        .connect(name, tcp)
        .await
        .map_err(|e| SocketError::Tls(e.to_string()))
}

/// The upgrade: the request with the deflate offer, and the venue's answer checked. Answers the
/// extensions the venue accepted and any frame bytes read with the answer.
async fn upgrade(
    io: &mut Transport,
    target: &Target,
) -> Result<(Option<String>, Option<Bytes>), SocketError> {
    let handshake_failed = |why: String| SocketError::Handshake(why);
    let key = handshake::generate_key();
    let request = handshake::build_request_with_headers(
        &target.authority,
        &target.resource,
        &key,
        None,
        Some(DEFLATE_OFFER),
        None,
    )
    .map_err(|e| handshake_failed(e.to_string()))?;
    io.write_all(&request).await.map_err(|e| handshake_failed(e.to_string()))?;
    io.flush().await.map_err(|e| handshake_failed(e.to_string()))?;
    let mut buffer = BytesMut::with_capacity(4096);
    loop {
        if buffer.len() > MAX_ANSWER_BYTES {
            return Err(handshake_failed("the answer is too large".into()));
        }
        let read = io.read_buf(&mut buffer).await.map_err(|e| handshake_failed(e.to_string()))?;
        if read == 0 {
            return Err(handshake_failed("the venue closed during the upgrade".into()));
        }
        let parsed =
            handshake::parse_response(&buffer).map_err(|e| handshake_failed(e.to_string()))?;
        let Some((answer, consumed)) = parsed else { continue };
        let accept = answer.accept.ok_or_else(|| handshake_failed("no accept key".into()))?;
        if !handshake::validate_accept_key(&key, accept) {
            return Err(handshake_failed("the accept key does not match".into()));
        }
        let extensions = answer.extensions.map(str::to_string);
        let leftover =
            (consumed < buffer.len()).then(|| Bytes::copy_from_slice(&buffer[consumed..]));
        return Ok((extensions, leftover));
    }
}

/// The deflate parameters the venue accepted. An extension the client did not offer, or deflate
/// parameters it cannot honour, fail the connection: the frames that follow could not be read.
fn accepted_deflate(extensions: Option<&str>) -> Result<Option<DeflateConfig>, SocketError> {
    let Some(extensions) = extensions.map(str::trim).filter(|e| !e.is_empty()) else {
        return Ok(None);
    };
    let unreadable = || {
        SocketError::Handshake(format!("the venue accepted an extension not offered: {extensions}"))
    };
    let params = parse_deflate_offer(extensions).ok_or_else(unreadable)?;
    DeflateConfig::from_params(&params).map(Some).map_err(|_| unreadable())
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
