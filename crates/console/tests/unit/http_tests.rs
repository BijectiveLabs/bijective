// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The bounds on what the console reads from a connection.

use super::*;

fn read(raw: &str) -> Result<Request, Unanswerable> {
    read_request(raw.as_bytes(), 64)
}

#[test]
fn a_form_post_is_read_with_its_body_and_headers() {
    let raw = "POST /control HTTP/1.1\r\nHost: 127.0.0.1:7333\r\nOrigin: http://127.0.0.1:7333\r\n\
               Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 11\r\n\r\naction=halt";
    let request = read(raw).expect("read");
    assert_eq!((request.method.as_str(), request.target.as_str()), ("POST", "/control"));
    assert_eq!(request.body, "action=halt");
    assert!(request.is_own_form());
}

/// A page on another site can make a browser post here. Its origin gives it away.
#[test]
fn a_form_from_another_origin_or_encoding_is_not_ours() {
    let raw = "POST /control HTTP/1.1\r\nHost: 127.0.0.1:7333\r\nOrigin: https://elsewhere.example\r\n\
               Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 0\r\n\r\n";
    assert!(!read(raw).expect("read").is_own_form());
    let raw = "POST /control HTTP/1.1\r\nHost: h\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n";
    assert!(!read(raw).expect("read").is_own_form());
}

#[test]
fn a_body_longer_than_the_bound_is_refused_unread() {
    let raw = "POST /control HTTP/1.1\r\nContent-Length: 65\r\n\r\n";
    assert_eq!(read(raw), Err(Unanswerable::TooLarge));
}

#[test]
fn a_head_longer_than_any_browsers_is_refused() {
    let raw = format!("GET / HTTP/1.1\r\nX-Padding: {}\r\n\r\n", "a".repeat(9 * 1024));
    assert_eq!(read(&raw), Err(Unanswerable::TooLarge));
}

#[test]
fn a_query_value_is_found_by_its_name() {
    let request = read("GET /control?issued=12 HTTP/1.1\r\n\r\n").expect("read");
    assert_eq!(request.query("issued"), Some("12"));
    assert_eq!(request.query("refused"), None);
}
