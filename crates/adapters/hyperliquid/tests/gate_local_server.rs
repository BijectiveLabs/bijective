// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The REST client's own limits against a local HTTP server that counts what reaches it.
//! A held call is one the server never sees, so the count is the evidence. The read path is used
//! because it needs no signature.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_secrets_broker::Credential;
use bijective_venue::normalise::Scale;
use bijective_venue_hyperliquid::rest::{ClientSetup, RestClient};

struct Stub {
    endpoint: String,
    hits: Arc<AtomicUsize>,
}

fn answer(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let mut text = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("Connection: close\r\n\r\n");
    text.push_str(body);
    text
}

/// Answers in order. The last answer repeats.
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
            // The body follows the headers. One more read drains a small one before the answer.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
            let _ = stream.read(&mut chunk);
            let index = counted.fetch_add(1, Ordering::SeqCst).min(answers.len() - 1);
            let _ = stream.write_all(answers[index].as_bytes());
        }
    });
    Stub { endpoint, hits }
}

fn client(stub: &Stub) -> RestClient {
    let key = Credential::new(vec![1u8; 32]);
    RestClient::new(ClientSetup {
        endpoint: &stub.endpoint,
        wallet_key: &key,
        account_address: "0x0000000000000000000000000000000000000001",
        price: Scale::new(2).expect("price scale"),
        quantity: Scale::new(3).expect("quantity scale"),
        timeout: Duration::from_secs(5),
        clock: Arc::new(WallClock::new()),
        mainnet: false,
    })
    .expect("a client")
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(future)
}

#[test]
fn five_server_errors_open_the_read_breaker_and_the_server_never_sees_the_rest() {
    let stub = serve(vec![answer(500, &[], "")]);
    let client = client(&stub);

    let results: Vec<_> = (0..8).map(|_| block_on(client.positions())).collect();

    assert!(results.iter().all(Result::is_err), "no read can succeed against a failing server");
    assert_eq!(stub.hits.load(Ordering::SeqCst), 5, "a held read reaches nobody");
}

#[test]
fn a_429_with_retry_after_holds_the_next_read() {
    let stub = serve(vec![answer(429, &[("Retry-After", "7")], "")]);
    let client = client(&stub);

    let first = block_on(client.positions());
    let second = block_on(client.positions());

    assert!(first.is_err() && second.is_err());
    assert_eq!(stub.hits.load(Ordering::SeqCst), 1, "the second read waited for the venue");
}

/// A venue that answers is healthy even when the answer is a refusal of the request itself.
#[test]
fn a_client_error_is_an_answer_and_never_opens_the_breaker() {
    let stub = serve(vec![answer(422, &[], "")]);
    let client = client(&stub);

    for _ in 0..12 {
        let _ = block_on(client.positions());
    }

    assert_eq!(stub.hits.load(Ordering::SeqCst), 12);
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

/// The shared checks an execution side must pass, against a local server. The server answers the
/// asset list, an accepted order, a refusal naming a repeated client identifier, and an empty
/// account. The venue's own text for a repeated identifier is not documented, so this proves how
/// the adapter reads that text, not what the venue sends.
#[test]
fn the_execution_side_passes_the_shared_conformance_checks() {
    use bijective_venue::contract::conformance::check_execution;
    use bijective_venue::contract::{Clearance, OrderRequest, Side, VenueSymbol};
    use bijective_venue_hyperliquid::rest::Execution;

    let stub = serve(vec![
        answer(200, &[], r#"{"universe":[{"name":"BTC","szDecimals":5,"maxLeverage":40}]}"#),
        answer(
            200,
            &[],
            r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":77}}]}}}"#,
        ),
        answer(
            200,
            &[],
            r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"error":"Order already placed with this cloid"}]}}}"#,
        ),
        answer(200, &[], r#"{"assetPositions":[]}"#),
    ]);
    let mut execution = Execution::new(client(&stub), declaration());
    let clearance = Clearance::granted(0, 0, true, 1).expect("cleared");
    let order = OrderRequest {
        client_id: "0x1234567890abcdef1234567890abcdef".to_string(),
        instrument: VenueSymbol::new("BTC"),
        side: Side::Buy,
        quantity_scaled: 1_000,
        price_scaled: Some(2_654_375),
        reduce_only: false,
        terms: Default::default(),
    };

    let results = block_on(check_execution(&mut execution, &clearance, &order, 1));

    assert_conformant(&results);
}

/// A body is read up to a bound. This account read is a valid empty answer padded far past it:
/// read whole it would be an empty account, read to the bound it is an unreadable read, and a read
/// that cannot be understood must never be taken for a flat book.
#[test]
fn an_oversized_body_is_a_failed_read_and_never_an_empty_account() {
    let padding = " ".repeat(9 * 1024 * 1024);
    let body = format!(r#"{{"assetPositions":[]}}{padding}"#);
    let stub = serve(vec![answer(200, &[], &body)]);
    let client = client(&stub);

    let read = block_on(client.positions());

    assert!(read.is_err(), "{read:?}");
}
