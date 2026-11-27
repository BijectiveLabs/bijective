// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The execution side against a local HTTP server that answers by route and keeps every request
//! it saw. Answers are the documented shapes, not captured venue traffic: no account exists yet,
//! so these prove how the adapter reads the documents.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bijective_common::clock::SequenceClock;
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{
    Clearance, OrderRequest, Recorded, Side, VenueExecution, VenueSymbol,
};
use bijective_venue::normalise::{Rejection, Scale, VenueOutcome};
use bijective_venue_alpaca::rest::{ClientSetup, Execution, RestClient};

/// The account read beside the positions, at the fields the margin reads.
const ACCOUNT: &str = r#"{"currency": "USD", "equity": "1000.00", "maintenance_margin": "0"}"#;

const NOW_NS: i128 = 1_794_819_035_000_000_000;
const ORDER_ID: &str = "61e69015-8549-4bfd-b9c3-01e75843f47d";

/// The venue's clock in the core session until after every test's instant.
const CORE: &str = r#"{"clocks":[{"is_market_day":true,"market":{"acronym":"NYSE","mic":"XNYS","name":"New York Stock Exchange","timezone":"America/New_York"},"next_market_close":"2026-11-16T21:00:00Z","next_market_open":"2026-11-17T14:30:00Z","phase":"core","phase_until":"2026-11-16T21:00:00Z","timestamp":"2026-11-16T15:00:00Z"}]}"#;

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: String,
}

/// An answer for a route, or `None` to hold the connection without answering.
type Route = Box<dyn Fn(&Seen) -> Option<String> + Send + Sync>;

struct Stub {
    endpoint: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn answer(status: u16, body: &str) -> Option<String> {
    Some(format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Seen> {
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(at) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        seen.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&seen[..end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: BTreeMap<String, String> = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
        .collect();
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while seen.len() < end + length {
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        seen.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&seen[end..end + length]).to_string();
    Some(Seen { method, target, headers, body })
}

fn serve(route: Route) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
    let endpoint = format!("http://{}", listener.local_addr().expect("an address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let route = Arc::new(route);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else { continue };
            let (route, kept) = (Arc::clone(&route), Arc::clone(&kept));
            // One thread per connection: an unanswered request must not hold the next one.
            std::thread::spawn(move || {
                let Some(request) = read_request(&mut stream) else { return };
                kept.lock().expect("the log").push(request.clone());
                match route(&request) {
                    Some(text) => drop(stream.write_all(text.as_bytes())),
                    None => std::thread::sleep(Duration::from_secs(2)),
                }
            });
        }
    });
    Stub { endpoint, seen }
}

fn client(stub: &Stub, timeout: Duration) -> RestClient {
    let secret = Credential::new(b"secret-key".to_vec());
    RestClient::new(ClientSetup {
        endpoint: &stub.endpoint,
        key_id: "key-id",
        secret: &secret,
        price: Scale::new(4).expect("price scale"),
        quantity: Scale::new(0).expect("quantity scale"),
        timeout,
        clock: Arc::new(SequenceClock::starting_at(NOW_NS)),
    })
    .expect("a client")
}

/// Ten shares at 150.12.
fn order(client_id: &str) -> OrderRequest {
    OrderRequest {
        client_id: client_id.to_string(),
        instrument: VenueSymbol::new("AAPL"),
        side: Side::Buy,
        quantity_scaled: 10,
        price_scaled: Some(1_501_200),
        reduce_only: false,
        terms: Default::default(),
    }
}

fn order_object(client_id: &str, status: &str, filled: &str) -> String {
    format!(
        r#"{{"id":"{ORDER_ID}","client_order_id":"{client_id}","symbol":"AAPL","side":"buy","qty":"10","filled_qty":"{filled}","status":"{status}","updated_at":"2026-11-17T14:30:12.418372Z"}}"#
    )
}

const FILL: &str = r#"[{"id":"20261117143013001::2d7be4ff-d1f3-43e9-856a-0f5cf5c5088e","activity_type":"FILL","transaction_time":"2026-11-17T14:30:13.001Z","type":"fill","price":"150.12","qty":"10","side":"buy","symbol":"AAPL","leaves_qty":"0","order_id":"61e69015-8549-4bfd-b9c3-01e75843f47d","cum_qty":"10","order_status":"filled"}]"#;

const POSITION: &str = r#"[{"symbol":"AAPL","qty":"10","side":"long","avg_entry_price":"150.12"}]"#;

/// Shaped as the reference's non-trade activity. The sign follows the account's own convention.
const FEE: &str = r#"[{"id":"20261118000000000::5f596936-6f23-4cef-bdf1-3806aae57dbf","activity_type":"FEE","activity_sub_type":"TAF","date":"2026-11-18","created_at":"2026-11-18T06:00:00Z","net_amount":"-0.02","currency":"USD","status":"executed"}]"#;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(future)
}

fn seen(stub: &Stub) -> Vec<Seen> {
    stub.seen.lock().expect("the log").clone()
}

#[test]
fn a_submit_carries_the_key_headers_and_its_cancel_names_the_venue_s_order() {
    let accepted = order_object("bl-1", "accepted", "0");
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/v3/clock") => answer(200, CORE),
        ("POST", "/v2/orders") => answer(200, &accepted),
        ("DELETE", t) if t == format!("/v2/orders/{ORDER_ID}") => answer(204, ""),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    let outcome = block_on(client.submit(&order("bl-1"))).expect("built");
    let again = block_on(client.submit(&order("bl-1"))).expect("built");
    let cancel = block_on(client.cancel("bl-1"));

    assert_eq!(outcome, VenueOutcome::Accepted);
    assert_eq!(again, VenueOutcome::Rejected(Rejection::Duplicate));
    assert_eq!(cancel, VenueOutcome::Accepted);
    let requests: Vec<Seen> =
        seen(&stub).into_iter().filter(|r| !r.target.starts_with("/v3/clock")).collect();
    assert_eq!(requests.len(), 2, "the duplicate sent nothing: {requests:?}");
    assert_eq!(requests[0].headers["apca-api-key-id"], "key-id");
    assert_eq!(requests[0].headers["apca-api-secret-key"], "secret-key");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).expect("JSON");
    assert_eq!(body["client_order_id"], "bl-1");
    assert_eq!(requests[1].method, "DELETE");
}

/// A submit that is never answered is unknown. The next evidence read asks for the order by its
/// client identifier, and its fill and the position are linked back to it.
#[test]
fn an_unanswered_submit_is_found_by_its_client_identifier_in_the_evidence() {
    let filled = order_object("bl-1", "filled", "10");
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/v3/clock") => answer(200, CORE),
        ("POST", _) => None,
        ("GET", t) if t.starts_with("/v2/orders?") => answer(200, "[]"),
        ("GET", t) if t.starts_with("/v2/orders:by_client_order_id?client_order_id=bl-1") => {
            answer(200, &filled)
        }
        ("GET", t) if t.starts_with("/v2/account/activities/FILL?") => answer(200, FILL),
        ("GET", t) if t.starts_with("/v2/account/activities/FEE?") => answer(200, FEE),
        ("GET", "/v2/positions") => answer(200, POSITION),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_millis(300));

    let outcome = block_on(client.submit(&order("bl-1"))).expect("built");
    let (evidence, refused) = block_on(client.evidence(NOW_NS - 60_000_000_000)).expect("read");

    assert!(matches!(outcome, VenueOutcome::Unknown(_)), "{outcome:?}");
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(evidence.orders.len(), 1);
    assert_eq!(evidence.orders[0].client_id.as_deref(), Some("bl-1"));
    assert_eq!(evidence.fills.len(), 1);
    assert_eq!(evidence.fills[0].client_id.as_deref(), Some("bl-1"));
    assert_eq!(evidence.charges.len(), 1);
    assert_eq!(evidence.charges[0].fee.amount_scaled, 2_000_000, "0.02 USD paid");
    assert!(evidence.complete);
    let fills = seen(&stub).into_iter().find(|r| r.target.contains("activities")).expect("read");
    assert!(fills.target.contains("after=2026-11-16T08:49:34Z"), "{}", fills.target);

    block_on(client.evidence(NOW_NS)).expect("read");
    let asked = seen(&stub).iter().filter(|r| r.target.contains("by_client_order_id")).count();
    assert_eq!(asked, 1, "a finished order is not asked about again");
}

/// After a restart the record hands the order back. A cancel finds it by its client identifier.
#[test]
fn a_restored_order_is_cancelled_through_its_client_identifier() {
    let open = order_object("bl-1", "new", "0");
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/v3/clock") => answer(200, CORE),
        ("GET", t) if t.starts_with("/v2/orders:by_client_order_id") => answer(200, &open),
        ("DELETE", t) if t == format!("/v2/orders/{ORDER_ID}") => answer(204, ""),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));
    client.adopt(&[Recorded {
        request: order("bl-1"),
        intent_ns: NOW_NS,
        venue_order_id: None,
        terminal: false,
    }]);

    assert_eq!(block_on(client.cancel("bl-1")), VenueOutcome::Accepted);
    let again = block_on(client.submit(&order("bl-1"))).expect("answered");
    assert_eq!(again, VenueOutcome::Rejected(Rejection::Duplicate));
}

/// A fill of an order no read has shown is linked by reading that order once. A refused read
/// leaves the evidence incomplete rather than the fill unclaimed.
#[test]
fn a_fill_of_an_unseen_order_reads_the_order_once() {
    let finished = order_object("bl-9", "filled", "10");
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/v3/clock") => answer(200, CORE),
        ("GET", t) if t.starts_with("/v2/orders?") => answer(200, "[]"),
        ("GET", t) if t == format!("/v2/orders/{ORDER_ID}") => answer(200, &finished),
        ("GET", t) if t.starts_with("/v2/account/activities/FILL?") => answer(200, FILL),
        ("GET", t) if t.starts_with("/v2/account/activities/FEE?") => answer(200, "[]"),
        ("GET", "/v2/positions") => answer(200, "[]"),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    let (evidence, refused) = block_on(client.evidence(NOW_NS)).expect("read");
    block_on(client.evidence(NOW_NS)).expect("read");

    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(evidence.fills[0].client_id.as_deref(), Some("bl-9"));
    let asked = seen(&stub).iter().filter(|r| r.target == format!("/v2/orders/{ORDER_ID}")).count();
    assert_eq!(asked, 1);
}

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

/// The shared checks an execution side must pass, on the documented order and position shapes.
#[test]
fn the_execution_side_passes_the_shared_conformance_checks() {
    use bijective_venue::contract::conformance::{CheckOutcome, check_execution};

    let accepted = order_object("bl-1", "accepted", "0");
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/v3/clock") => answer(200, CORE),
        ("POST", "/v2/orders") => answer(200, &accepted),
        ("GET", "/v2/positions") => answer(200, POSITION),
        ("GET", "/v2/account") => answer(200, ACCOUNT),
        _ => answer(404, "{}"),
    }));
    let mut execution = Execution::new(client(&stub, Duration::from_secs(5)), declaration());
    let clearance = Clearance::granted(0, 0, true, 1).expect("cleared");

    let results = block_on(check_execution(&mut execution, &clearance, &order("bl-1"), NOW_NS));

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
    let account = block_on(execution.account(0)).expect("an account");
    assert_eq!(account.positions[0].quantity_scaled, 10);
    assert_eq!(account.positions[0].entry_price_scaled, 1_501_200);
}

/// Outside the core session an entry is refused without a request. An exit is still sent.
#[test]
fn an_entry_waits_for_the_core_session_and_an_exit_does_not() {
    let closed = CORE.replace(r#""phase":"core""#, r#""phase":"pre""#);
    let accepted = order_object("bl-2", "accepted", "0");
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/v3/clock") => answer(200, &closed),
        ("POST", "/v2/orders") => answer(200, &accepted),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    let entry = block_on(client.submit(&order("bl-1"))).expect("built");
    let exit = OrderRequest {
        client_id: "bl-2".into(),
        price_scaled: None,
        reduce_only: true,
        side: Side::Sell,
        ..order("bl-2")
    };
    let sent = block_on(client.submit(&exit)).expect("built");

    let VenueOutcome::Rejected(Rejection::Unclassified { venue_code }) = entry else {
        panic!("refused: {entry:?}");
    };
    assert!(venue_code.contains("outside the core session"), "{venue_code}");
    assert_eq!(sent, VenueOutcome::Accepted);
    let posts = seen(&stub).iter().filter(|r| r.method == "POST").count();
    assert_eq!(posts, 1, "only the exit was sent");
}
