// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Addresses and the venue answer to the deflate offer.

use super::*;

#[test]
fn an_address_names_its_host_port_and_resource() {
    let target = Target::parse("wss://stream.example.com/v5/public?x=1").expect("parsed");
    assert!(target.secure);
    assert_eq!((target.host.as_str(), target.port), ("stream.example.com", 443));
    assert_eq!(target.authority, "stream.example.com");
    assert_eq!(target.resource, "/v5/public?x=1");
    let local = Target::parse("ws://127.0.0.1:9001/ws").expect("parsed");
    assert!(!local.secure);
    assert_eq!((local.port, local.authority.as_str()), (9001, "127.0.0.1:9001"));
    assert!(matches!(Target::parse("https://example.com"), Err(SocketError::Address(_))));
}

#[test]
fn no_extension_is_a_plain_connection_and_deflate_is_read_with_its_parameters() {
    assert_eq!(accepted_deflate(None), Ok(None));
    assert_eq!(accepted_deflate(Some("")), Ok(None));
    let accepted = accepted_deflate(Some("permessage-deflate; server_no_context_takeover"))
        .expect("readable")
        .expect("deflate");
    assert!(accepted.server_no_context_takeover);
}

#[test]
fn an_extension_not_offered_fails_the_connection() {
    let answered = accepted_deflate(Some("x-webkit-deflate-frame"));
    assert!(matches!(answered, Err(SocketError::Handshake(_))), "{answered:?}");
}
