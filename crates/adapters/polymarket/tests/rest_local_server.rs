// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The execution side against a local HTTP server that answers by route and keeps every request
//! it saw. The clock is fixed, so the order hash the server must name is computed here the same
//! way the client computes it. Answers are the documented shapes, not captured venue traffic: no
//! funded account exists yet, so these prove how the adapter reads the documents.

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
use bijective_venue::normalise::{Rejection, Scale, Unknown, VenueOutcome};
use bijective_venue_polymarket::order::{Maker, sign};
use bijective_venue_polymarket::rest::{ApiKey, ClientSetup, Execution, RequestError, RestClient};
use bijective_venue_polymarket::signing::{Exchange, Signer, request_signature};

const TOKEN: &str = "32338220190071351435772801779725302244575775216413325951443816017994629993401";
const SECRET: &str = "c2VjcmV0LWtleS0xMjM0NTY3ODkwMTIzNDU2Nzg5MDE=";
const NOW_NS: i128 = 1_794_819_035_000_000_000;
const KEY: [u8; 32] = [7u8; 32];

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
    let wallet = Credential::new(KEY.to_vec());
    let secret = Credential::new(SECRET.as_bytes().to_vec());
    let passphrase = Credential::new(b"phrase".to_vec());
    RestClient::new(ClientSetup {
        endpoint: &stub.endpoint,
        data_endpoint: &stub.endpoint,
        wallet_key: &wallet,
        deposit_wallet: None,
        api: ApiKey { key: "api-key", secret: &secret, passphrase: &passphrase },
        price: Scale::new(4).expect("price scale"),
        quantity: Scale::new(6).expect("quantity scale"),
        timeout,
        clock: Arc::new(SequenceClock::starting_at(NOW_NS)),
    })
    .expect("a client")
}

/// Ten shares at 0.52.
fn order(client_id: &str) -> OrderRequest {
    OrderRequest {
        client_id: client_id.to_string(),
        instrument: VenueSymbol::new(TOKEN),
        side: Side::Buy,
        quantity_scaled: 10_000_000,
        price_scaled: Some(5_200),
        reduce_only: false,
        terms: Default::default(),
    }
}

/// The hash the client signs for `order` on a CTF book without negative risk.
fn hash_of(order: &OrderRequest) -> String {
    let signer = Signer::from_secret(&KEY).expect("key");
    let maker = Maker { address: signer.address(), signature_type: 0, exchange: Exchange::Ctf };
    sign(order, &maker, &signer, "api-key", NOW_NS).expect("signed").order_id
}

const BOOK: &str = r#"{"asset_id":"x","bids":[],"asks":[],"tick_size":"0.001","neg_risk":false}"#;

fn accepted(hash: &str) -> Option<String> {
    answer(200, &format!(r#"{{"success":true,"orderID":"{hash}","status":"live","errorMsg":""}}"#))
}

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
fn a_submit_reads_the_book_once_and_sends_the_order_under_verifiable_l2_headers() {
    let hash = hash_of(&order("bl-1"));
    let named = hash.clone();
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/book?") => answer(200, BOOK),
        ("POST", "/order") => accepted(&named),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    let outcome = block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");

    assert_eq!(outcome, VenueOutcome::Accepted);
    let requests = seen(&stub);
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[0].target, format!("/book?token_id={TOKEN}"));
    let post = &requests[1];
    let timestamp: u64 = post.headers["poly_timestamp"].parse().expect("seconds");
    let expected =
        request_signature(SECRET, timestamp, "POST", "/order", &post.body).expect("hmac");
    assert_eq!(post.headers["poly_signature"], expected);
    assert_eq!(post.headers["poly_api_key"], "api-key");
    assert_eq!(post.headers["poly_passphrase"], "phrase");
    let address = format!(
        "0x{}",
        bijective_venue_polymarket::signing::hex(
            &Signer::from_secret(&KEY).expect("key").address(),
        )
    );
    assert!(post.headers["poly_address"].eq_ignore_ascii_case(&address));
    assert_eq!(timestamp, 1_794_819_035);
}

#[test]
fn the_same_client_identifier_is_never_signed_twice() {
    let hash = hash_of(&order("bl-1"));
    let stub = serve(Box::new(move |r| match r.method.as_str() {
        "GET" => answer(200, BOOK),
        _ => accepted(&hash),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");
    let again = block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");

    assert_eq!(again, VenueOutcome::Rejected(Rejection::Duplicate));
    assert_eq!(seen(&stub).iter().filter(|r| r.method == "POST").count(), 1);
}

#[test]
fn an_answer_naming_another_order_is_unknown_and_never_accepted() {
    let stub = serve(Box::new(|r| match r.method.as_str() {
        "GET" => answer(200, BOOK),
        _ => accepted("0x00"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    let outcome = block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");

    assert_eq!(outcome, VenueOutcome::Unknown(Unknown::Unreadable));
}

#[test]
fn a_venue_refusal_frees_the_identifier_because_no_order_exists() {
    let stub = serve(Box::new(|r| match r.method.as_str() {
        "GET" => answer(200, BOOK),
        _ => answer(400, r#"{"error":"order 0xabc is invalid. Price breaks minimum tick size"}"#),
    }));
    let mut client = client(&stub, Duration::from_secs(5));

    let first = block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");
    let cancel = block_on(client.cancel("bl-1"));

    assert!(matches!(first, VenueOutcome::Rejected(Rejection::Unclassified { .. })), "{first:?}");
    assert!(cancel.is_err(), "no hash is held for a refused order: {cancel:?}");
}

#[test]
fn a_cancel_sends_the_exact_signed_body_and_reads_the_canceled_list() {
    let hash = hash_of(&order("bl-1"));
    let listed = hash.clone();
    let stub = serve(Box::new(move |r| match r.method.as_str() {
        "GET" => answer(200, BOOK),
        "POST" => accepted(&listed),
        _ => answer(200, &format!(r#"{{"canceled":["{listed}"],"not_canceled":{{}}}}"#)),
    }));
    let mut client = client(&stub, Duration::from_secs(5));
    block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");

    let outcome = block_on(client.cancel("bl-1")).expect("built");

    assert_eq!(outcome, VenueOutcome::Accepted);
    let delete = seen(&stub).into_iter().find(|r| r.method == "DELETE").expect("a cancel");
    assert_eq!(delete.target, "/order");
    assert_eq!(delete.body, format!(r#"{{"orderID":"{hash}"}}"#));
}

#[test]
fn a_cancel_for_an_order_this_process_did_not_sign_is_refused_without_a_request() {
    let stub = serve(Box::new(|_| answer(500, "")));
    let mut execution = Execution::new(client(&stub, Duration::from_secs(5)), declaration());

    let outcome = block_on(execution.cancel(&VenueSymbol::new(TOKEN), "bl-unknown", 0));

    assert!(matches!(outcome, VenueOutcome::Rejected(Rejection::Unclassified { .. })));
    assert!(seen(&stub).is_empty());
}

#[test]
fn the_breaker_holds_submits_after_five_server_errors() {
    let stub = serve(Box::new(|r| match r.method.as_str() {
        "GET" => answer(200, BOOK),
        _ => answer(500, r#"{"error":"could not insert order"}"#),
    }));
    let mut execution = Execution::new(client(&stub, Duration::from_secs(5)), declaration());
    let clearance = Clearance::granted(0, 0, true, 1).expect("cleared");

    let outcomes: Vec<VenueOutcome> = (0..7)
        .map(|i| block_on(execution.submit(&clearance, &order(&format!("bl-{i}")), NOW_NS)))
        .collect();

    assert!(outcomes[..5].iter().all(|o| matches!(o, VenueOutcome::Unknown(_))), "{outcomes:?}");
    assert!(outcomes[5..].iter().all(|o| *o == VenueOutcome::NotSent), "{outcomes:?}");
    assert_eq!(seen(&stub).iter().filter(|r| r.method == "POST").count(), 5);
}

/// A submit that is never answered is unknown. The next evidence read asks for the order by the
/// hash signed before the call, and the venue's answer links it back to the client identifier.
#[test]
fn an_unanswered_submit_is_unknown_and_its_order_is_found_by_hash_in_the_evidence() {
    let hash = hash_of(&order("bl-1"));
    let row = format!(
        r#"{{"id":"{hash}","status":"LIVE","asset_id":"{TOKEN}","side":"BUY","original_size":"10","size_matched":"0","price":"0.52","created_at":1794819035}}"#
    );
    let trade = format!(
        r#"{{"data":[{{"id":"t-1","taker_order_id":"{hash}","asset_id":"{TOKEN}","side":"BUY","size":"4","price":"0.52","status":"TRADE_STATUS_CONFIRMED","match_time":"1794819036","trader_side":"TAKER","maker_orders":[]}}],"next_cursor":"LTE="}}"#
    );
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/book?") => answer(200, BOOK),
        ("POST", _) => None,
        ("GET", t) if t.starts_with("/data/orders") => {
            answer(200, r#"{"data":[],"next_cursor":"LTE="}"#)
        }
        ("GET", t) if t.starts_with("/data/order/") => answer(200, &row),
        ("GET", t) if t.starts_with("/data/trades") => answer(200, &trade),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_millis(300));

    let outcome = block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");
    let (evidence, refused) = block_on(client.evidence(NOW_NS - 60_000_000_000)).expect("read");

    assert!(matches!(outcome, VenueOutcome::Unknown(_)), "{outcome:?}");
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(evidence.orders.len(), 1);
    assert_eq!(evidence.orders[0].client_id.as_deref(), Some("bl-1"));
    assert_eq!(evidence.fills.len(), 1);
    assert_eq!(evidence.fills[0].client_id.as_deref(), Some("bl-1"));
    assert_eq!(evidence.fills[0].fill.quantity_scaled, 4_000_000);
    assert!(evidence.complete);
    let trades =
        seen(&stub).into_iter().find(|r| r.target.starts_with("/data/trades")).expect("read");
    assert!(trades.target.contains("after=1794818974"), "{}", trades.target);
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

/// The shared checks an execution side must pass. The account is the data service's documented
/// page with one holding.
#[test]
fn the_execution_side_passes_the_shared_conformance_checks() {
    use bijective_venue::contract::conformance::{CheckOutcome, check_execution};

    let hash = hash_of(&order("bl-1"));
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/book?") => answer(200, BOOK),
        ("GET", t) if t.starts_with("/v2/positions?") => answer(
            200,
            &format!(
                r#"{{"data":[{{"token_id":"{TOKEN}","current_size":125.5,"avg_price":0.42}}],"pagination":{{"limit":500,"offset":0,"has_more":false,"next_cursor":null}}}}"#
            ),
        ),
        ("POST", _) => accepted(&hash),
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
    assert_eq!(account.positions[0].quantity_scaled, 125_500_000);
    assert_eq!(account.positions[0].entry_price_scaled, 4_200);
}

/// With a Deposit Wallet the order and every list read are the wallet's, and the request is made
/// in the name of the key that owns the API key.
#[test]
fn a_deposit_wallet_is_the_maker_and_the_key_stays_the_requester() {
    const WALLET: &str = "0x2e234dae75c793f67a35089c9d99245e1c58470b";
    let stub = serve(Box::new(|r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/book?") => answer(200, BOOK),
        ("POST", _) => answer(200, r#"{"success":false,"errorMsg":"not funded"}"#),
        _ => answer(200, r#"{"data":[],"next_cursor":"LTE="}"#),
    }));
    let key = Credential::new(KEY.to_vec());
    let secret = Credential::new(SECRET.as_bytes().to_vec());
    let passphrase = Credential::new(b"phrase".to_vec());
    let mut client = RestClient::new(ClientSetup {
        endpoint: &stub.endpoint,
        data_endpoint: &stub.endpoint,
        wallet_key: &key,
        deposit_wallet: Some(WALLET),
        api: ApiKey { key: "api-key", secret: &secret, passphrase: &passphrase },
        price: Scale::new(4).expect("price scale"),
        quantity: Scale::new(6).expect("quantity scale"),
        timeout: Duration::from_secs(5),
        clock: Arc::new(SequenceClock::starting_at(NOW_NS)),
    })
    .expect("a client");

    block_on(client.submit(&order("bl-1"), NOW_NS)).expect("built");
    block_on(client.evidence(NOW_NS)).expect("read");

    let requests = seen(&stub);
    let post = requests.iter().find(|r| r.method == "POST").expect("an order");
    let body: serde_json::Value = serde_json::from_str(&post.body).expect("json");
    assert_eq!(body["order"]["maker"], WALLET);
    assert_eq!(body["order"]["signer"], WALLET);
    assert_eq!(body["order"]["signatureType"], 3);
    let key_address = bijective_venue_polymarket::signing::checksummed(
        &Signer::from_secret(&KEY).expect("key").address(),
    );
    assert_eq!(post.headers["poly_address"], key_address);
    let trades = requests.iter().find(|r| r.target.starts_with("/data/trades")).expect("read");
    assert!(trades.target.contains(&format!("maker_address={WALLET}")), "{}", trades.target);
}

fn restored(client_id: &str, terminal: bool) -> Recorded {
    Recorded { request: order(client_id), intent_ns: NOW_NS, venue_order_id: None, terminal }
}

const NO_ROWS: &str = r#"{"data":[],"next_cursor":"LTE="}"#;

/// After a restart the record hands the order back: the venue's live list names it by the hash
/// signed before the restart, the other candidates are dropped, and a cancel reaches it.
#[test]
fn a_restored_order_is_named_by_the_evidence_and_can_then_be_cancelled() {
    let hash = hash_of(&order("bl-1"));
    let row = format!(
        r#"{{"data":[{{"id":"{hash}","status":"LIVE","asset_id":"{TOKEN}","side":"BUY","original_size":"10","size_matched":"0","price":"0.52","created_at":1794819035}}],"next_cursor":"LTE="}}"#
    );
    let canceled = format!(r#"{{"canceled":["{hash}"],"not_canceled":{{}}}}"#);
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/data/orders") => answer(200, &row),
        ("GET", t) if t.starts_with("/data/trades") => answer(200, NO_ROWS),
        ("DELETE", "/order") => answer(200, &canceled),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));
    client.adopt(&[restored("bl-1", false)]);

    let early = block_on(client.cancel("bl-1"));
    assert!(matches!(early, Err(RequestError::UnresolvedOrder)), "{early:?}");
    assert!(seen(&stub).is_empty(), "nothing is sent before the venue names the order");
    let again = block_on(client.submit(&order("bl-1"), NOW_NS)).expect("answered");
    assert_eq!(again, VenueOutcome::Rejected(Rejection::Duplicate));

    let (evidence, refused) = block_on(client.evidence(NOW_NS)).expect("read");
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(evidence.orders.len(), 1);
    assert_eq!(evidence.orders[0].client_id.as_deref(), Some("bl-1"));
    let asked = seen(&stub).iter().filter(|r| r.target.starts_with("/data/order/")).count();
    assert_eq!(asked, 2, "the two other candidates are asked for once");

    let cancel = block_on(client.cancel("bl-1")).expect("sent");
    assert_eq!(cancel, VenueOutcome::Accepted);
    let delete = seen(&stub).into_iter().find(|r| r.method == "DELETE").expect("sent");
    assert_eq!(delete.body, format!(r#"{{"orderID":"{hash}"}}"#));

    block_on(client.evidence(NOW_NS)).expect("read");
    let asked = seen(&stub).iter().filter(|r| r.target.starts_with("/data/order/")).count();
    assert_eq!(asked, 2, "a dropped candidate is never asked about again");
}

/// A finished order is never asked about one by one, and its trade still names it as this
/// platform's rather than external.
#[test]
fn a_restored_finished_order_keeps_its_trades_its_own_without_reads() {
    let hash = hash_of(&order("bl-1"));
    let trade = format!(
        r#"{{"data":[{{"id":"t-1","taker_order_id":"{hash}","asset_id":"{TOKEN}","side":"BUY","size":"10","price":"0.52","status":"TRADE_STATUS_CONFIRMED","match_time":"1794819036","trader_side":"TAKER","maker_orders":[]}}],"next_cursor":"LTE="}}"#
    );
    let stub = serve(Box::new(move |r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/data/orders") => answer(200, NO_ROWS),
        ("GET", t) if t.starts_with("/data/trades") => answer(200, &trade),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));
    client.adopt(&[restored("bl-1", true)]);

    let (evidence, refused) = block_on(client.evidence(NOW_NS - 60_000_000_000)).expect("read");

    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(evidence.fills.len(), 1);
    assert_eq!(evidence.fills[0].client_id.as_deref(), Some("bl-1"));
    assert!(seen(&stub).iter().all(|r| !r.target.starts_with("/data/order/")));
}

/// An identifier the venue already named is held under that hash alone.
#[test]
fn a_restored_order_the_venue_named_needs_no_candidates() {
    let hash = hash_of(&order("bl-1"));
    let canceled = format!(r#"{{"canceled":["{hash}"],"not_canceled":{{}}}}"#);
    let stub = serve(Box::new(move |r| match r.method.as_str() {
        "DELETE" => answer(200, &canceled),
        _ => answer(404, "{}"),
    }));
    let mut client = client(&stub, Duration::from_secs(5));
    let named =
        Recorded { venue_order_id: Some(hash.to_ascii_uppercase()), ..restored("bl-1", false) };
    client.adopt(&[named]);

    assert_eq!(block_on(client.cancel("bl-1")).expect("sent"), VenueOutcome::Accepted);
}
