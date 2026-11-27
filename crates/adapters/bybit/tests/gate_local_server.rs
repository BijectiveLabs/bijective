// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The REST client's own limits against a local HTTP server that counts what reaches it.
//! A held call is one the server never sees, so the count is the evidence.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_secrets_broker::credential::Credential;
use bijective_venue::contract::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::{Rejection, Scale, Unknown, VenueOutcome};
use bijective_venue_bybit::rest::RestClient;

/// What the stub answers with, in order. The last answer repeats.
struct Stub {
    endpoint: String,
    hits: Arc<AtomicUsize>,
}

/// The account read after the positions: one unified row.
const WALLET: &str = r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"accountType":"UNIFIED","totalMarginBalance":"1000","totalMaintenanceMargin":"0"}]}}"#;

fn answer(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let mut text = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("Connection: close\r\n\r\n");
    text.push_str(body);
    text
}

fn serve(answers: Vec<String>) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
    let endpoint = format!("http://{}", listener.local_addr().expect("an address"));
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else { continue };
            let mut seen = Vec::new();
            let mut chunk = [0u8; 4096];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen.extend_from_slice(&chunk[..n]),
                }
            }
            let index = counted.fetch_add(1, Ordering::SeqCst).min(answers.len() - 1);
            let _ = stream.write_all(answers[index].as_bytes());
        }
    });
    Stub { endpoint, hits }
}

fn client(stub: &Stub) -> RestClient {
    RestClient::new(
        &stub.endpoint,
        "key",
        Credential::new(b"secret".to_vec()),
        bijective_venue::profile::InstrumentScales::uniform_over(
            ["BTCUSDT", "ETHUSDT"],
            bijective_venue::profile::Scales {
                price: Scale::new(2).expect("price scale"),
                quantity: Scale::new(3).expect("quantity scale"),
            },
        ),
        Duration::from_secs(5),
        Arc::new(WallClock::new()),
    )
    .expect("a client")
}

fn order() -> OrderRequest {
    OrderRequest {
        client_id: "bl-test-1".to_string(),
        instrument: VenueSymbol::new("BTCUSDT"),
        side: Side::Buy,
        quantity_scaled: 1_000,
        price_scaled: Some(2_654_375),
        reduce_only: false,
        terms: Default::default(),
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(future)
}

/// Sends the order `times` times in a row and returns each outcome.
fn submit_times(client: &RestClient, times: usize) -> Vec<VenueOutcome> {
    (0..times).map(|_| block_on(client.submit(&order())).expect("a request was built")).collect()
}

#[test]
fn the_breaker_stops_submits_after_five_server_errors_and_the_server_never_sees_the_rest() {
    let stub = serve(vec![answer(500, &[], "")]);
    let client = client(&stub);

    let outcomes = submit_times(&client, 8);

    let sent = &outcomes[..5];
    assert!(sent.iter().all(|o| *o == VenueOutcome::Unknown(Unknown::Unreadable)), "{sent:?}");
    assert!(outcomes[5..].iter().all(|o| *o == VenueOutcome::NotSent), "{outcomes:?}");
    assert_eq!(stub.hits.load(Ordering::SeqCst), 5, "a held submit reaches nobody");
}

/// An exit must always be attempted, even with the order path shut.
#[test]
fn a_cancel_still_reaches_the_venue_while_the_submit_breaker_is_open() {
    let stub = serve(vec![answer(500, &[], "")]);
    let client = client(&stub);
    submit_times(&client, 8);
    let before = stub.hits.load(Ordering::SeqCst);

    let outcome = block_on(client.cancel(&VenueSymbol::new("BTCUSDT"), "bl-test-1"));

    assert_eq!(stub.hits.load(Ordering::SeqCst), before + 1, "the cancel was sent");
    assert_ne!(outcome, VenueOutcome::NotSent);
}

#[test]
fn a_429_with_retry_after_is_carried_on_the_outcome_and_holds_reads() {
    let stub = serve(vec![answer(429, &[("Retry-After", "7")], "")]);
    let client = client(&stub);

    let outcome = block_on(client.submit(&order())).expect("a request was built");
    let read = block_on(client.positions("USDT"));

    assert_eq!(
        outcome,
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(7_000_000_000) })
    );
    assert!(read.is_err(), "the read was held for the venue's seven seconds");
    assert_eq!(stub.hits.load(Ordering::SeqCst), 1, "the held read reached nobody");
}

/// The venue's own page: 403 is an IP block of at least ten minutes.
#[test]
fn a_403_is_an_ip_block_that_holds_reads_for_ten_minutes() {
    let stub = serve(vec![answer(403, &[], "")]);
    let client = client(&stub);

    let outcome = block_on(client.submit(&order())).expect("a request was built");
    let read = block_on(client.positions("USDT"));

    assert_eq!(
        outcome,
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(600_000_000_000) })
    );
    assert!(read.is_err());
    assert_eq!(stub.hits.load(Ordering::SeqCst), 1);
}

/// Seven seconds from a throttled answer and a hung server are different things. The second is
/// what the breaker is for, and a refusal of the request itself is neither.
#[test]
fn a_venue_refusal_is_an_answer_and_never_opens_the_breaker() {
    let refusal = answer(200, &[], r#"{"retCode":110007,"retMsg":"insufficient balance"}"#);
    let stub = serve(vec![refusal]);
    let client = client(&stub);

    let outcomes = submit_times(&client, 12);

    assert!(outcomes.iter().all(|o| *o != VenueOutcome::NotSent), "{outcomes:?}");
    assert_eq!(stub.hits.load(Ordering::SeqCst), 12);
}

/// The venue answers HTTP 200 with its own rate-limit code (error-code page, read 2026-11-05:
/// 10018 "Exceeded the IP Rate Limit"). Continuing risks the ten-minute IP block, so reads wait
/// one 5-second window, the window the venue's limit is stated over.
#[test]
fn a_rate_limit_code_in_a_200_answer_holds_reads_for_a_window() {
    let limited = answer(200, &[], r#"{"retCode":10018,"retMsg":"Exceeded the IP Rate Limit."}"#);
    let stub = serve(vec![limited]);
    let client = client(&stub);

    let first = block_on(client.positions("USDT"));
    let second = block_on(client.positions("USDT"));

    assert!(first.is_err() && second.is_err());
    assert_eq!(stub.hits.load(Ordering::SeqCst), 1, "the second read waited for the window");
}

/// A declaration serving depth and nothing else. The checks do not read it.
fn declaration() -> bijective_venue::profile::CapabilityDeclaration {
    let profile = serde_json::json!({
        "format": 1,
        "venue": "the venue under check",
        "verified_at": null,
        "transports": { "stream": {
            "funding": { "model": "published", "interval_ns": "28800000000000" },
            "data_classes": ["depth"],
            "max_staleness_ns": "40000000000"
        }},
        "reduce_only": "fill-time-clamp",
        "order_types": [],
        "instruments": [],
        "identifier_rules": { "allowed": "abc", "max_len": 36 },
        "transformations": [],
        "support": { "venue": "the venue under check", "depth": "data-only", "debuggable_to_root_cause": true }
    });
    let profile = bijective_venue::profile::VenueProfile::from_json(&profile.to_string())
        .expect("the profile");
    bijective_venue::profile::CapabilityDeclaration::from_profile(&profile)
}

/// What the three checks that run without a live venue must show.
fn assert_conformant(results: &[bijective_venue::contract::conformance::CheckResult]) {
    use bijective_venue::contract::conformance::CheckOutcome;

    for name in [
        "resubmission-is-idempotent",
        "unknown-is-not-a-rejection",
        "account-readable-for-reconciliation",
    ] {
        assert!(
            results.iter().any(|r| r.name == name && matches!(r.outcome, CheckOutcome::Passed)),
            "{name} did not pass: {results:?}"
        );
    }
    assert!(
        !results.iter().any(|r| matches!(r.outcome, CheckOutcome::Failed { .. })),
        "{results:?}"
    );
}

/// The shared checks an execution side must pass, against a local server that answers as the
/// venue documents: accepted, then "OrderLinkedID is duplicate" (110072) for the same identifier,
/// then an empty position page. Before 110072 was mapped, the second answer read as a rejection
/// of an order the venue holds and this failed.
#[test]
fn the_execution_side_passes_the_shared_conformance_checks() {
    use bijective_venue::contract::Clearance;
    use bijective_venue::contract::conformance::check_execution;
    use bijective_venue_bybit::rest::Execution;

    let stub = serve(vec![
        answer(
            200,
            &[],
            r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"1","orderLinkId":"bl-test-1"}}"#,
        ),
        answer(200, &[], r#"{"retCode":110072,"retMsg":"OrderLinkedID is duplicate"}"#),
        answer(200, &[], r#"{"retCode":0,"retMsg":"OK","result":{"list":[],"nextPageCursor":""}}"#),
        answer(200, &[], WALLET),
    ]);
    let mut execution = Execution::new(client(&stub), declaration(), "USDT");
    let clearance = Clearance::granted(0, 0, true, 1).expect("cleared");

    let results = block_on(check_execution(&mut execution, &clearance, &order(), 1));

    assert_conformant(&results);
}

/// A body is read up to a bound. This one is a valid accepted answer padded far past it: read
/// whole it would be `Accepted`, read to the bound it is unreadable, which for a submit is an
/// outcome nobody may assume either way.
#[test]
fn an_oversized_body_is_unreadable_and_never_read_as_the_accepted_answer_inside_it() {
    let padding = " ".repeat(9 * 1024 * 1024);
    let body = format!(r#"{{"retCode":0,"retMsg":"OK"}}{padding}"#);
    let stub = serve(vec![answer(200, &[], &body)]);
    let client = client(&stub);

    let outcome = block_on(client.submit(&order())).expect("a request was built");

    assert_eq!(outcome, VenueOutcome::Unknown(Unknown::Unreadable));
}
