// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Runs the execution side against the venue: account, evidence, a resting limit order far from
//! the market, the venue's answer to a reused client identifier and to an unknown one, a cancel,
//! and the evidence after it. The key id and the secret key are read from standard input, one per
//! line, never from an argument. Nothing secret is printed.
//! Usage: execution_probe <endpoint> <symbol> <limit-price-scaled-at-4>

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::{Clock, WallClock};
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::Scale;
use bijective_venue_alpaca::rest::{ClientSetup, RestClient};

fn line(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> String {
    let text = lines.next().and_then(Result::ok).unwrap_or_default();
    text.trim().trim_matches('"').trim_matches('\'').to_string()
}

fn client(endpoint: &str, key_id: &str, secret: &Credential) -> RestClient {
    RestClient::new(ClientSetup {
        endpoint,
        key_id,
        secret,
        price: Scale::new(4).expect("price scale"),
        quantity: Scale::new(0).expect("quantity scale"),
        timeout: Duration::from_secs(10),
        clock: Arc::new(WallClock::new()),
    })
    .expect("a client")
}

async fn show_evidence(client: &mut RestClient, since_ns: i128, client_id: &str) {
    match client.evidence(since_ns).await {
        Ok((evidence, refused)) => {
            println!(
                "evidence: {} order(s), {} fill(s), complete {}, {} refused row(s)",
                evidence.orders.len(),
                evidence.fills.len(),
                evidence.complete,
                refused.len()
            );
            for row in &refused {
                println!("  refused: {row}");
            }
            for order in
                evidence.orders.iter().filter(|o| o.client_id.as_deref() == Some(client_id))
            {
                println!(
                    "  probe order: {:?}, filled {}",
                    order.status, order.filled_quantity_scaled
                );
            }
        }
        Err(e) => println!("evidence refused: {e}"),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, endpoint, symbol, price] = args.as_slice() else {
        eprintln!("usage: execution_probe <endpoint> <symbol> <limit-price-scaled-at-4>");
        std::process::exit(2);
    };
    let price_scaled: i128 = price.parse().expect("a scaled price");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let key_id = line(&mut lines);
    let secret = Credential::new(line(&mut lines).into_bytes());

    let mut probe = client(endpoint, &key_id, &secret);
    let now = WallClock::new().now_ns();
    let since = now - 86_400_000_000_000;
    match probe.positions().await {
        Ok(account) => println!("account: {} position(s)", account.positions.len()),
        Err(e) => println!("account refused: {e}"),
    }
    show_evidence(&mut probe, since, "").await;

    let client_id = format!("bl-probe-{}", now / 1_000_000);
    let order = OrderRequest {
        client_id: client_id.clone(),
        instrument: VenueSymbol::new(symbol.as_str()),
        side: Side::Buy,
        quantity_scaled: 1,
        price_scaled: Some(price_scaled),
        reduce_only: false,
        terms: Default::default(),
    };
    println!("submit: {:?}", probe.submit(&order).await);
    show_evidence(&mut probe, since, &client_id).await;

    // A second client has no memory of the first submit, so the venue answers the reuse itself.
    let mut fresh = client(endpoint, &key_id, &secret);
    println!("reused client identifier: {:?}", fresh.submit(&order).await);
    println!("cancel of an unknown identifier: {:?}", fresh.cancel("bl-probe-none").await);

    println!("cancel: {:?}", probe.cancel(&client_id).await);
    tokio::time::sleep(Duration::from_secs(2)).await;
    show_evidence(&mut probe, since, &client_id).await;
}
