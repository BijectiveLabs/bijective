// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One order cycle through the adapter the node uses, trade updates attached. `fill` buys at a
//! marketable limit, follows push and evidence until settled, cancels any remainder, and closes
//! with a reduce-only market sell. `rest` leaves a resting order and exits. `resume` adopts it in a
//! new client and cancels it. Key id and secret come on standard input. Nothing secret is printed.
//! Usage: equity_cycle_probe <endpoint> <symbol> <phase>, where phase is
//! `fill <limit-price> <shares>`, `rest <limit-price>` or
//! `resume <client-id> <intent-ns> <limit-price>`, prices scaled at 4.

use std::io::BufRead;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bijective_common::clock::{Clock, WallClock};
use bijective_lifecycle::report::{ReportedStatus, VenueEvidence};
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{OrderRequest, Recorded, Side, VenueSymbol};
use bijective_venue::normalise::Scale;
use bijective_venue_alpaca::rest::{ClientSetup, RestClient};
use bijective_venue_alpaca::trade_stream::{LinkSetup, TradeLink, endpoint_for};

/// How long one order is followed before the probe stops waiting for it.
const FOLLOW_BOUND: Duration = Duration::from_secs(60);

struct Keys {
    key_id: String,
    secret: String,
}

fn read_keys() -> Keys {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut line = || {
        let text = lines.next().and_then(Result::ok).unwrap_or_default();
        text.trim().trim_matches('"').trim_matches('\'').to_string()
    };
    Keys { key_id: line(), secret: line() }
}

fn client(endpoint: &str, keys: &Keys) -> RestClient {
    let secret = Credential::new(keys.secret.clone().into_bytes());
    RestClient::new(ClientSetup {
        endpoint,
        key_id: &keys.key_id,
        secret: &secret,
        price: Scale::new(4).expect("price scale"),
        quantity: Scale::new(0).expect("quantity scale"),
        timeout: Duration::from_secs(10),
        clock: Arc::new(WallClock::new()),
    })
    .expect("a client")
}

fn start_push_link(endpoint: &str, keys: &Keys) -> Option<TradeLink> {
    let url = endpoint_for(endpoint)?;
    let setup = LinkSetup {
        url: url.to_string(),
        key_id: keys.key_id.clone(),
        secret: keys.secret.clone(),
        clock: Arc::new(WallClock::new()),
    };
    TradeLink::start(setup).ok()
}

fn now_ns() -> i128 {
    WallClock::new().now_ns()
}

fn request(
    client_id: &str,
    symbol: &str,
    side: Side,
    shares: i128,
    price: Option<i128>,
) -> OrderRequest {
    OrderRequest {
        client_id: client_id.to_string(),
        instrument: VenueSymbol::new(symbol),
        side,
        quantity_scaled: shares,
        price_scaled: price,
        reduce_only: price.is_none(),
        terms: Default::default(),
    }
}

/// The probe order's line in the evidence, and its fills. `None` when the evidence lacks it.
fn report(evidence: &VenueEvidence, client_id: &str) -> Option<(ReportedStatus, i128)> {
    for fill in evidence.fills.iter().filter(|f| f.client_id.as_deref() == Some(client_id)) {
        let f = &fill.fill;
        println!(
            "    fill {} qty {} price {} fee {:?} liquidity {:?} at {:?}",
            f.fill_id, f.quantity_scaled, f.price_scaled, f.fee, f.liquidity, f.exchange_time
        );
    }
    let order = evidence.orders.iter().find(|o| o.client_id.as_deref() == Some(client_id))?;
    Some((order.status, order.filled_quantity_scaled))
}

fn is_settled(status: ReportedStatus) -> bool {
    !matches!(status, ReportedStatus::Working)
}

/// Reads the evidence each second, and at once when the link says something changed, until the
/// order settles or the bound passes. Returns the last status and filled quantity seen.
async fn follow(
    client: &mut RestClient,
    push_link: Option<&TradeLink>,
    since_ns: i128,
    client_id: &str,
) -> Option<(ReportedStatus, i128)> {
    let start = Instant::now();
    let mut last = None;
    while start.elapsed() < FOLLOW_BOUND {
        let pushed = push_link.map(TradeLink::pushed);
        match client.evidence(since_ns).await {
            Ok((evidence, refused)) => {
                let seen = report(&evidence, client_id);
                println!(
                    "{:>6} ms push {pushed:?} order {seen:?} complete {} refused {}",
                    start.elapsed().as_millis(),
                    evidence.complete,
                    refused.len()
                );
                last = seen;
            }
            Err(e) => println!("evidence refused: {e}"),
        }
        if last.is_some_and(|(status, _)| is_settled(status)) {
            return last;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    last
}

async fn fill_cycle(endpoint: &str, symbol: &str, price: i128, shares: i128, keys: &Keys) {
    let push_link = start_push_link(endpoint, keys);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut probe = client(endpoint, keys);
    let since = now_ns() - 60_000_000_000;
    let entry_id = format!("bl-cycle-{}", now_ns() / 1_000_000);
    let entry = request(&entry_id, symbol, Side::Buy, shares, Some(price));
    println!("entry submit: {:?}", probe.submit(&entry).await);
    let settled = follow(&mut probe, push_link.as_ref(), since, &entry_id).await;
    let filled = settled.map_or(0, |(_, filled)| filled);
    if !settled.is_some_and(|(status, _)| is_settled(status)) {
        println!("entry remainder cancel: {:?}", probe.cancel(&entry_id).await);
        follow(&mut probe, push_link.as_ref(), since, &entry_id).await;
    }
    if filled > 0 {
        let exit_id = format!("bl-cycle-exit-{}", now_ns() / 1_000_000);
        let exit = request(&exit_id, symbol, Side::Sell, filled, None);
        println!("exit submit: {:?}", probe.submit(&exit).await);
        follow(&mut probe, push_link.as_ref(), since, &exit_id).await;
    }
    match probe.positions().await {
        Ok(account) => println!("positions after: {:?}", account.positions),
        Err(e) => println!("account refused: {e}"),
    }
}

async fn rest_order(endpoint: &str, symbol: &str, price: i128, keys: &Keys) {
    let mut probe = client(endpoint, keys);
    let intent_ns = now_ns();
    let client_id = format!("bl-rest-{}", intent_ns / 1_000_000);
    let order = request(&client_id, symbol, Side::Buy, 1, Some(price));
    println!("submit: {:?}", probe.submit(&order).await);
    println!("left resting: client id {client_id} intent ns {intent_ns}");
}

async fn resume(
    endpoint: &str,
    symbol: &str,
    client_id: &str,
    intent_ns: i128,
    price: i128,
    keys: &Keys,
) {
    let push_link = start_push_link(endpoint, keys);
    let mut probe = client(endpoint, keys);
    let recorded = Recorded {
        request: request(client_id, symbol, Side::Buy, 1, Some(price)),
        intent_ns,
        venue_order_id: None,
        terminal: false,
    };
    probe.adopt(&[recorded]);
    let since = intent_ns - 60_000_000_000;
    println!("adopted; evidence before cancel:");
    let start = Instant::now();
    if let Ok((evidence, _)) = probe.evidence(since).await {
        println!("  order {:?}", report(&evidence, client_id));
    }
    println!(
        "resubmit of the adopted identifier: {:?}",
        probe.submit(&request(client_id, symbol, Side::Buy, 1, Some(price))).await
    );
    println!(
        "cancel: {:?} after {} ms",
        probe.cancel(client_id).await,
        start.elapsed().as_millis()
    );
    follow(&mut probe, push_link.as_ref(), since, client_id).await;
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let keys = read_keys();
    let number = |text: &str| -> i128 { text.parse().expect("a number") };
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [_, endpoint, symbol, "fill", price, shares] => {
            fill_cycle(endpoint, symbol, number(price), number(shares), &keys).await;
        }
        [_, endpoint, symbol, "rest", price] => {
            rest_order(endpoint, symbol, number(price), &keys).await
        }
        [_, endpoint, symbol, "resume", client_id, intent_ns, price] => {
            resume(endpoint, symbol, client_id, number(intent_ns), number(price), &keys).await;
        }
        _ => {
            eprintln!("usage: see the header of examples/equity_cycle_probe.rs");
            std::process::exit(2);
        }
    }
}
