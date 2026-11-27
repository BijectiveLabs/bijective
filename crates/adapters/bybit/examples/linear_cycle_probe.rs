// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One order cycle on a linear USDT contract through the adapter the node uses, private stream
//! attached. `fill`, `close`, `rest` and `resume` as in the equity probe of the Alpaca adapter.
//! `resume` also answers a reuse of the identifier. Key and secret come on standard input. Nothing
//! secret is printed. Prices and quantities are scaled integers at the given scales.
//! Usage: linear_cycle_probe <endpoint> <symbol> <price-scale> <qty-scale> <phase>
//! where phase is `fill <limit-price> <qty>`, `close <qty>`, `rest <limit-price> <qty>` or
//! `resume <client-id> <limit-price> <qty>`.

use std::io::BufRead;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bijective_common::clock::{Clock, WallClock};
use bijective_lifecycle::report::{ReportedStatus, VenueEvidence};
use bijective_secrets_broker::credential::Credential;
use bijective_venue::contract::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::Scale;
use bijective_venue_bybit::evidence::Scales;
use bijective_venue_bybit::private_stream::{LinkSetup, PrivateLink, endpoint_for};
use bijective_venue_bybit::rest::{DEFAULT_TIMEOUT, RestClient};

/// How long one order is followed before the probe stops waiting for it.
const FOLLOW_BOUND: Duration = Duration::from_secs(60);
const SETTLE_COIN: &str = "USDT";

struct Probe {
    endpoint: String,
    symbol: String,
    scales: Scales,
    api_key: String,
    secret: String,
}

impl Probe {
    fn client(&self) -> RestClient {
        RestClient::new(
            self.endpoint.clone(),
            self.api_key.clone(),
            Credential::new(self.secret.clone().into_bytes()),
            self.instrument_scales(),
            DEFAULT_TIMEOUT,
            Arc::new(WallClock::new()),
        )
        .expect("a client")
    }

    fn instrument_scales(&self) -> bijective_venue::profile::InstrumentScales {
        bijective_venue::profile::InstrumentScales::uniform_over(
            [self.symbol.as_str()],
            self.scales,
        )
    }

    fn start_push_link(&self) -> Option<PrivateLink> {
        let url = endpoint_for(&self.endpoint)?;
        let setup = LinkSetup {
            url: url.to_string(),
            api_key: self.api_key.clone(),
            secret: Credential::new(self.secret.clone().into_bytes()),
            scales: self.instrument_scales(),
            clock: Arc::new(WallClock::new()),
        };
        PrivateLink::start(setup).ok()
    }

    fn request(&self, client_id: &str, side: Side, qty: i128, price: Option<i128>) -> OrderRequest {
        OrderRequest {
            client_id: client_id.to_string(),
            instrument: VenueSymbol::new(self.symbol.as_str()),
            side,
            quantity_scaled: qty,
            price_scaled: price,
            reduce_only: price.is_none(),
            terms: Default::default(),
        }
    }

    fn instrument(&self) -> VenueSymbol {
        VenueSymbol::new(self.symbol.as_str())
    }
}

fn now_ns() -> i128 {
    WallClock::new().now_ns()
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

/// Reads the evidence each second until the order settles or the bound passes, printing what the
/// private stream said before each read. Returns the last status and filled quantity seen.
async fn follow(
    probe: &Probe,
    client: &RestClient,
    push_link: Option<&PrivateLink>,
    since_ns: i128,
    client_id: &str,
) -> Option<(ReportedStatus, i128)> {
    let start = Instant::now();
    let catalogue = [probe.instrument()];
    let mut last = None;
    while start.elapsed() < FOLLOW_BOUND {
        let pushed = push_link.map(PrivateLink::pushed);
        match client.evidence(SETTLE_COIN, &catalogue, since_ns, now_ns()).await {
            Ok((evidence, refused)) => {
                let seen = report(&evidence, client_id);
                println!(
                    "{:>6} ms push {pushed:?} order {seen:?} complete {} refused {}",
                    start.elapsed().as_millis(),
                    evidence.complete,
                    refused.len()
                );
                for row in &refused {
                    println!("    refused: {row}");
                }
                last = seen;
            }
            Err(e) => println!("evidence refused: {e}"),
        }
        if last.is_some_and(|(status, _)| status != ReportedStatus::Working) {
            return last;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    last
}

async fn fill_cycle(probe: &Probe, price: i128, qty: i128) {
    let push_link = probe.start_push_link();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let client = probe.client();
    let since = now_ns() - 60_000_000_000;
    let entry_id = format!("bl-cycle-{}", now_ns() / 1_000_000);
    let entry = probe.request(&entry_id, Side::Buy, qty, Some(price));
    println!("entry submit: {:?}", client.submit(&entry).await);
    let settled = follow(probe, &client, push_link.as_ref(), since, &entry_id).await;
    let filled = settled.map_or(0, |(_, filled)| filled);
    if !settled.is_some_and(|(status, _)| status != ReportedStatus::Working) {
        println!(
            "entry remainder cancel: {:?}",
            client.cancel(&probe.instrument(), &entry_id).await
        );
        follow(probe, &client, push_link.as_ref(), since, &entry_id).await;
    }
    if filled > 0 {
        let exit_id = format!("bl-cycle-exit-{}", now_ns() / 1_000_000);
        let exit = probe.request(&exit_id, Side::Sell, filled, None);
        println!("exit submit: {:?}", client.submit(&exit).await);
        follow(probe, &client, push_link.as_ref(), since, &exit_id).await;
    }
    match client.positions(SETTLE_COIN).await {
        Ok(account) => println!("positions after: {:?}", account.positions),
        Err(e) => println!("account refused: {e}"),
    }
}

/// Closes a long with a reduce-only market sell and follows it.
async fn close(probe: &Probe, qty: i128) {
    let push_link = probe.start_push_link();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let client = probe.client();
    let since = now_ns() - 60_000_000_000;
    let exit_id = format!("bl-cycle-exit-{}", now_ns() / 1_000_000);
    let exit = probe.request(&exit_id, Side::Sell, qty, None);
    println!("exit submit: {:?}", client.submit(&exit).await);
    follow(probe, &client, push_link.as_ref(), since, &exit_id).await;
    match client.positions(SETTLE_COIN).await {
        Ok(account) => println!("positions after: {:?}", account.positions),
        Err(e) => println!("account refused: {e}"),
    }
}

async fn rest_order(probe: &Probe, price: i128, qty: i128) {
    let client = probe.client();
    let client_id = format!("bl-rest-{}", now_ns() / 1_000_000);
    let order = probe.request(&client_id, Side::Buy, qty, Some(price));
    println!("submit: {:?}", client.submit(&order).await);
    println!("left resting: client id {client_id}");
}

async fn resume(probe: &Probe, client_id: &str, price: i128, qty: i128) {
    let push_link = probe.start_push_link();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let client = probe.client();
    let since = now_ns() - 3_600_000_000_000;
    let catalogue = [probe.instrument()];
    if let Ok((evidence, _)) = client.evidence(SETTLE_COIN, &catalogue, since, now_ns()).await {
        println!("before cancel: order {:?}", report(&evidence, client_id));
    }
    let again = probe.request(client_id, Side::Buy, qty, Some(price));
    println!("reuse of the identifier: {:?}", client.submit(&again).await);
    println!("cancel: {:?}", client.cancel(&probe.instrument(), client_id).await);
    follow(probe, &client, push_link.as_ref(), since, client_id).await;
}

fn read_keys() -> (String, String) {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut line = || {
        let text = lines.next().and_then(Result::ok).unwrap_or_default();
        text.trim().trim_matches('"').trim_matches('\'').to_string()
    };
    (line(), line())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let number = |text: &str| -> i128 { text.parse().expect("a number") };
    let scale = |text: &str| Scale::new(text.parse().expect("a scale")).expect("a scale");
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let [_, endpoint, symbol, price_scale, qty_scale, phase @ ..] = words.as_slice() else {
        eprintln!("usage: see the header of examples/linear_cycle_probe.rs");
        std::process::exit(2);
    };
    let (api_key, secret) = read_keys();
    let probe = Probe {
        endpoint: (*endpoint).to_string(),
        symbol: (*symbol).to_string(),
        scales: Scales { price: scale(price_scale), quantity: scale(qty_scale) },
        api_key,
        secret,
    };
    match phase {
        ["fill", price, qty] => fill_cycle(&probe, number(price), number(qty)).await,
        ["close", qty] => close(&probe, number(qty)).await,
        ["rest", price, qty] => rest_order(&probe, number(price), number(qty)).await,
        ["resume", client_id, price, qty] => {
            resume(&probe, client_id, number(price), number(qty)).await;
        }
        _ => {
            eprintln!("usage: see the header of examples/linear_cycle_probe.rs");
            std::process::exit(2);
        }
    }
}
