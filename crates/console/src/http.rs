// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The little HTTP the console speaks: one request per connection, read with bounds, answered
//! and closed. Anything outside those bounds is refused before it is interpreted.

use std::io::{BufRead, BufReader, Read, Write};

/// The request line and headers together. A browser's are far smaller.
const MAX_HEAD_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The target as sent, query included.
    pub target: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub content_type: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unanswerable {
    TooLarge,
    Malformed,
    Io(String),
}

/// Reads one request. Precondition: the stream has a read timeout, so a silent peer cannot hold
/// the console. A body is read only up to `max_body` bytes. A longer one is refused unread.
pub fn read_request(stream: impl Read, max_body: usize) -> Result<Request, Unanswerable> {
    let mut reader = BufReader::new(stream);
    let mut head_bytes = 0;
    let mut line = String::new();
    let mut next_line = |reader: &mut BufReader<_>, line: &mut String| {
        line.clear();
        let n = reader.read_line(line).map_err(|e| Unanswerable::Io(e.to_string()))?;
        head_bytes += n;
        if head_bytes > MAX_HEAD_BYTES {
            return Err(Unanswerable::TooLarge);
        }
        Ok(n)
    };

    next_line(&mut reader, &mut line)?;
    let mut words = line.split_whitespace();
    let (Some(method), Some(target)) = (words.next(), words.next()) else {
        return Err(Unanswerable::Malformed);
    };
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        host: None,
        origin: None,
        content_type: None,
        body: String::new(),
    };

    let mut length = 0_usize;
    while next_line(&mut reader, &mut line)? > 0 {
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(Unanswerable::Malformed);
        };
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => request.host = Some(value),
            "origin" => request.origin = Some(value),
            "content-type" => request.content_type = Some(value),
            "content-length" => length = value.parse().map_err(|_| Unanswerable::Malformed)?,
            _ => {}
        }
    }
    if length > max_body {
        return Err(Unanswerable::TooLarge);
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).map_err(|e| Unanswerable::Io(e.to_string()))?;
    request.body = String::from_utf8(body).map_err(|_| Unanswerable::Malformed)?;
    Ok(request)
}

impl Request {
    /// A form from a page this console served: the form's encoding, and an origin, when the
    /// browser sends one, naming the host the request was sent to.
    #[must_use]
    pub fn is_own_form(&self) -> bool {
        let encoded = self
            .content_type
            .as_deref()
            .is_some_and(|t| t.starts_with("application/x-www-form-urlencoded"));
        let same_origin = match (&self.origin, &self.host) {
            (None, _) => true,
            (Some(origin), Some(host)) => {
                origin.split_once("://").is_some_and(|(_, authority)| authority == host)
            }
            (Some(_), None) => false,
        };
        encoded && same_origin
    }

    /// The value of `name` in the target's query, as sent.
    #[must_use]
    pub fn query(&self, name: &str) -> Option<&str> {
        let (_, query) = self.target.split_once('?')?;
        query.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    }
}

/// The security headers on every answer. Forms may post only back to this console.
const POLICY: &str = "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; \
                      form-action 'self'; frame-ancestors 'none'\r\nX-Content-Type-Options: \
                      nosniff\r\nReferrer-Policy: no-referrer";

pub fn respond(stream: impl Write, status: &str, body: &str) -> std::io::Result<()> {
    respond_as(stream, status, "text/html; charset=utf-8", body)
}

pub fn respond_as(
    mut stream: impl Write,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\n{POLICY}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

pub const JSON: &str = "application/json";

/// A file of the served application (`app`), under the application's own policy.
pub fn respond_file(
    mut stream: impl Write,
    content_type: &str,
    policy: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\n{policy}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// The head of a Server-Sent Events answer. It has no length: the body ends when the connection
/// does.
pub fn open_events(mut stream: impl Write) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
         {POLICY}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

/// One event. Precondition: `data` is one line, as every `table` document is.
pub fn send_event(mut stream: impl Write, name: &str, data: &str) -> std::io::Result<()> {
    stream.write_all(format!("event: {name}\ndata: {data}\n\n").as_bytes())
}

/// A comment line, which a reader ignores, so an idle stream is not closed as dead on the way.
pub fn send_keep_alive(mut stream: impl Write) -> std::io::Result<()> {
    stream.write_all(b": keep-alive\n\n")?;
    stream.flush()
}

/// After a form, the browser is sent to read the result, so reloading it does not post again.
pub fn see_other(mut stream: impl Write, location: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 303 See Other\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Cache-Control: no-store\r\n{POLICY}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
#[path = "../tests/unit/http_tests.rs"]
mod tests;
