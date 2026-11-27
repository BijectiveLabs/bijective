// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Read-only reconciliation drill: signed REST account read, reconciled. Passphrase comes from stdin only.
//! Usage: venue_probe <keystore> <endpoint> [settle-coin] [instrument:qty:price ...]

use std::io::BufRead;

use bijective_reconciliation::{FallbackKey, Fidelity, PlatformPosition, VenuePosition, reconcile};
use bijective_secrets_broker::keystore::{Keystore, read_salt};
use bijective_secrets_broker::source::{DeclaredSource, KeySource, PassphraseSource};
use bijective_venue::normalise::Scale;
use bijective_venue_bybit::rest::RestClient;

/// A just-opened position may not be on both sides yet.
const IN_FLIGHT_WINDOW_NS: i128 = 30_000_000_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let keystore_path = args.next().ok_or("usage: venue_probe <keystore> <endpoint> [settle]")?;
    let endpoint = args.next().ok_or("an endpoint is required")?;
    let settle = args.next().unwrap_or_else(|| "USDT".to_string());

    let (api_key, secret) = credentials(&keystore_path)?;
    let client = RestClient::new(
        &endpoint,
        api_key,
        secret,
        // The probe's own two instruments. Any other symbol reads at the outside-catalogue scale.
        bijective_venue::profile::InstrumentScales::uniform_over(
            ["BTCUSDT", "ETHUSDT"],
            bijective_venue::profile::Scales { price: Scale::new(2)?, quantity: Scale::new(3)? },
        ),
        bijective_venue_bybit::rest::DEFAULT_TIMEOUT,
        std::sync::Arc::new(bijective_common::clock::WallClock::new()),
    )?;

    let ours = args.map(|spec| platform_position(&spec)).collect::<Result<Vec<_>, _>>()?;

    let snapshot =
        client.positions(&settle).await.map_err(|e| format!("the venue refused the read: {e}"))?;

    // Positions only. Never print the credential or anything derived from it.
    println!("the venue answered: {} open position(s)", snapshot.positions.len());
    for p in &snapshot.positions {
        println!(
            "  {:<12} qty {:>18}  entry {:>16}",
            p.instrument, p.quantity_scaled, p.entry_price_scaled
        );
    }

    let theirs: Vec<VenuePosition> = snapshot
        .positions
        .iter()
        .map(|p| VenuePosition::from_account(p, (snapshot.observed_at_ns / 1_000_000) as u64))
        .collect();

    // Positions carry no order ids, so instrument fallback is the fitting fidelity.
    let report = reconcile(
        &ours,
        &theirs,
        Fidelity::Fallback(FallbackKey::Instrument),
        IN_FLIGHT_WINDOW_NS,
        snapshot.observed_at_ns as u64,
        snapshot.observed_at_ns,
    );

    print_report(ours.len(), &report);
    Ok(())
}

/// The venue key pair from the keystore. The passphrase is read from standard input only, so it
/// never appears in a process listing or shell history.
fn credentials(
    keystore_path: &str,
) -> Result<(String, bijective_secrets_broker::credential::Credential), Box<dyn std::error::Error>>
{
    let mut passphrase = String::new();
    std::io::stdin().lock().read_line(&mut passphrase)?;
    let passphrase = passphrase.trim_end_matches(['\n', '\r']).as_bytes().to_vec();
    if passphrase.is_empty() {
        return Err("passphrase is empty".into());
    }

    let sealed = std::fs::read(keystore_path)?;
    let salt = read_salt(&sealed).map_err(|e| format!("not a keystore: {e:?}"))?;
    let key = PassphraseSource::new(passphrase, salt).unwrap_key()?;
    let keystore = Keystore::open(&sealed, &key, DeclaredSource::Passphrase)
        .map_err(|e| format!("could not open the keystore: {e:?}"))?;

    let api_key = keystore.get("venue-api-key").ok_or("venue-api-key is not in the keystore")?;
    let secret =
        keystore.get("venue-api-secret").ok_or("venue-api-secret is not in the keystore")?;

    let api_key = String::from_utf8(api_key.expose().to_vec())?;
    let secret = bijective_secrets_broker::credential::Credential::new(secret.expose().to_vec());
    Ok((api_key, secret))
}

/// `instrument:quantity:price`, both figures already in the venue's scale. A malformed figure is
/// refused rather than read as zero: a zero position reconciles against a flat venue as clear.
fn platform_position(spec: &str) -> Result<PlatformPosition, String> {
    let [instrument, quantity, price] = spec.split(':').collect::<Vec<_>>()[..] else {
        return Err(format!("a platform position is instrument:quantity:price, not {spec}"));
    };
    let figure = |text: &str| {
        text.parse::<i128>().map_err(|_| format!("{text} in {spec} is not a scaled integer"))
    };
    Ok(PlatformPosition {
        client_order_id: format!("bl-{instrument}"),
        instrument: bijective_venue::contract::VenueSymbol::new(instrument),
        venue_order_id: None,
        quantity_scaled: figure(quantity)?,
        average_price_scaled: figure(price)?,
        opened_at_ns: 0,
    })
}

fn print_report(platform_positions: usize, report: &bijective_reconciliation::Report) {
    println!();
    println!(
        "reconciled {} platform position(s) against the venue as of {}",
        platform_positions, report.as_of
    );
    if report.is_clear() {
        println!("  clear: {} matched, nothing diverged", report.matched);
    }
    for d in &report.divergences {
        println!("  {:<10} {:?}", d.instrument, d.class);
    }
    for defect in &report.profile_defects {
        println!("  PROFILE    {}", defect.detail);
    }
}
