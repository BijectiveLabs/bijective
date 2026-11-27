// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The evidence read against a local server answering in the venue's live shapes: what is asked,
//! in what order, and what the answers become.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_lifecycle::{PositionEvidence, ReportedStatus};
use bijective_secrets_broker::Credential;
use bijective_venue::contract::VenueSymbol;
use bijective_venue::normalise::Scale;
use bijective_venue_hyperliquid::evidence::Clients;
use bijective_venue_hyperliquid::rest::{ClientSetup, RestClient, cloid_for};
use serde_json::{Value, json};

const SINCE_MS: u64 = 1_795_000_000_000;
const NOW_MS: u64 = 1_795_000_600_000;

/// Answers in order, the last repeating, and keeps every request body.
fn serve(answers: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
    let endpoint = format!("http://{}", listener.local_addr().expect("an address"));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&bodies);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let mut seen = Vec::new();
            let mut chunk = [0u8; 65536];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&chunk[..n]);
            }
            let text = String::from_utf8_lossy(&seen).to_string();
            let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            let mut kept = kept.lock().expect("bodies");
            kept.push(body);
            let index = (kept.len() - 1).min(answers.len() - 1);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                answers[index].len(),
                answers[index]
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (endpoint, bodies)
}

fn client(endpoint: &str) -> RestClient {
    let key = Credential::new(vec![1u8; 32]);
    RestClient::new(ClientSetup {
        endpoint,
        wallet_key: &key,
        account_address: "0x0000000000000000000000000000000000000001",
        price: Scale::new(1).expect("price scale"),
        quantity: Scale::new(4).expect("quantity scale"),
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

fn order(cloid: &str, oid: u64, status: &str, sz: &str, at_ms: u64) -> Value {
    json!({
        "order": { "coin": "ETH", "side": "B", "limitPx": "1719.9", "sz": sz, "oid": oid,
                   "timestamp": at_ms, "origSz": "0.0100", "cloid": cloid },
        "status": status,
        "statusTimestamp": at_ms
    })
}

fn fill(cloid: &str, tid: u64, at_ms: u64) -> Value {
    json!({ "coin": "ETH", "px": "1720.0", "sz": "0.0100", "side": "B", "time": at_ms,
            "oid": 9, "crossed": true, "fee": "0.007740", "tid": tid, "cloid": cloid,
            "feeToken": "USDC" })
}

fn read(
    answers: Vec<Value>,
    clients: &Clients,
) -> (bijective_lifecycle::VenueEvidence, Vec<String>) {
    let (endpoint, bodies) = serve(answers.iter().map(Value::to_string).collect());
    let catalogue = [VenueSymbol::new("ETH")];
    let since_ns = i128::from(SINCE_MS) * 1_000_000;
    let now_ns = i128::from(NOW_MS) * 1_000_000;
    let (evidence, refused) =
        block_on(client(&endpoint).evidence(clients, &catalogue, since_ns, now_ns)).expect("read");
    assert!(refused.is_empty() || !evidence.complete, "a refusal must mark it incomplete");
    let bodies = bodies.lock().expect("bodies").clone();
    (evidence, bodies)
}

/// One order in the history, one the history no longer lists asked for by cloid, a foreign order
/// that finished before the window, a fill, a funding charge and an empty book.
#[test]
fn the_venue_records_become_complete_evidence() {
    let mut clients = Clients::default();
    clients.placed("bl-1");
    clients.placed("bl-2");
    let foreign = "0x0000000000000000000001a1f7197daa";
    let answers = vec![
        json!([
            order(&cloid_for("bl-1"), 1, "canceled", "0.0100", SINCE_MS + 20),
            order(&cloid_for("bl-1"), 1, "open", "0.0100", SINCE_MS + 10),
            order(foreign, 3, "canceled", "0.0100", SINCE_MS - 1),
        ]),
        json!({ "status": "order", "order": order(&cloid_for("bl-2"), 2, "filled", "0.0", SINCE_MS + 30) }),
        json!([fill(&cloid_for("bl-2"), 77, SINCE_MS + 30)]),
        json!([{ "time": SINCE_MS + 40, "hash": "0x00",
                 "delta": { "type": "funding", "coin": "ETH", "usdc": "0.000100", "szi": "-0.01",
                            "fundingRate": "0.00001", "nSamples": null } }]),
        json!({ "assetPositions": [] }),
    ];
    let (evidence, bodies) = read(answers, &clients);

    assert!(evidence.complete, "{evidence:?}");
    let states: Vec<_> =
        evidence.orders.iter().map(|o| (o.client_id.as_deref(), o.status)).collect();
    assert_eq!(
        states,
        [(Some("bl-1"), ReportedStatus::Canceled), (Some("bl-2"), ReportedStatus::Filled)]
    );
    assert_eq!(evidence.fills[0].client_id.as_deref(), Some("bl-2"));
    assert_eq!(evidence.charges[0].fee.amount_scaled, -100_000_000, "received is negative");
    let PositionEvidence::Reported(positions) = &evidence.positions else { panic!("positions") };
    assert_eq!(positions[0].quantity_scaled, 0, "a catalogue instrument left out is flat");

    let kinds: Vec<String> = bodies
        .iter()
        .map(|b| serde_json::from_str::<Value>(b).expect("json")["type"].to_string())
        .collect();
    let expected =
        ["historicalOrders", "orderStatus", "userFillsByTime", "userFunding", "clearinghouseState"];
    assert_eq!(kinds, expected.map(|k| format!("\"{k}\"")));
    assert!(bodies[1].contains(&format!("\"oid\":\"{}\"", cloid_for("bl-2"))), "{}", bodies[1]);
    assert!(bodies[2].contains(&format!("\"startTime\":{SINCE_MS},\"endTime\":{NOW_MS}")));
}

/// A full page is followed from its newest stamp. The stamp is inclusive, so the row it repeats
/// is kept once, and a page that adds nothing ends the read as complete.
#[test]
fn a_full_page_of_fills_is_followed_from_its_newest_stamp() {
    let first: Vec<Value> =
        (0..500).map(|i| fill(&cloid_for("bl-1"), 1000 + i, SINCE_MS + i)).collect();
    let newest = SINCE_MS + 499;
    let answers = vec![
        json!([]),
        Value::Array(first),
        json!([fill(&cloid_for("bl-1"), 1499, newest)]),
        json!([]),
        json!({ "assetPositions": [] }),
    ];
    let (evidence, bodies) = read(answers, &Clients::default());

    assert!(evidence.complete, "{evidence:?}");
    assert_eq!(evidence.fills.len(), 500);
    assert!(bodies[2].contains(&format!("\"startTime\":{newest}")), "{}", bodies[2]);
}

#[test]
fn a_row_that_cannot_be_judged_leaves_the_evidence_incomplete() {
    let answers = vec![
        json!([order(&cloid_for("bl-1"), 1, "somethingNew", "0.0100", SINCE_MS + 1)]),
        json!([]),
        json!([]),
        json!({ "assetPositions": [] }),
    ];
    let (evidence, _) = read(answers, &Clients::default());
    assert!(!evidence.complete);
}
