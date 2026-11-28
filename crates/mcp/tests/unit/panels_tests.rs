// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The console's answer is taken only when it is a success.

use super::body_of;

#[test]
fn a_200_answer_gives_its_body() {
    let answer = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"format\":1}";
    assert_eq!(body_of(answer).unwrap(), "{\"format\":1}");
}

#[test]
fn another_status_is_refused_with_the_console_words() {
    let answer = "HTTP/1.1 404 Not Found\r\n\r\nno such panel";
    let refused = body_of(answer).unwrap_err();
    assert!(refused.contains("404") && refused.contains("no such panel"));
}

#[test]
fn an_answer_without_a_body_is_refused() {
    assert!(body_of("HTTP/1.1 200 OK").is_err());
}
