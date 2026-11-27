// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Draft a venue profile from the venue's own published instrument rules. Needs no credential.
//! Usage: bijective-profile <venue> <instrument,...> <url|answer-file> [--settles-in <asset>]
//!        [--out <dir>]
//! The profile is written to standard output, or with `--out` to `<dir>/profile.json` beside a
//! starter node configuration for paper and backtest runs, `<dir>/config.json`. It is unverified.
//! `bijective-verify-venue` checks it.
//! Exit codes: 0 drafted, 1 the venue could not be read or drafted from, 2 usage.

use std::process::ExitCode;

use bijective_venue::profile::{FundingModel, Instrument, Transport};
use bijective_venue_assurance::published::{draft, instrument_from};

const USAGE: &str = "usage: bijective-profile <venue> <instrument,...> <url|answer-file> \
                     [--settles-in <asset>] [--out <dir>]";

/// What was asked for.
struct Asked {
    venue: String,
    symbols: String,
    source: String,
    settles_in: Option<String>,
    out: Option<std::path::PathBuf>,
}

fn parse() -> Option<Asked> {
    let mut args = std::env::args().skip(1);
    let (venue, symbols, source) = (args.next()?, args.next()?, args.next()?);
    let mut asked = Asked { venue, symbols, source, settles_in: None, out: None };
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--settles-in" => asked.settles_in = Some(args.next()?),
            "--out" => asked.out = Some(std::path::PathBuf::from(args.next()?)),
            _ => return None,
        }
    }
    Some(asked)
}

#[tokio::main]
async fn main() -> ExitCode {
    let Some(asked) = parse() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut instruments: Vec<Instrument> = Vec::new();
    let mut funding: Option<FundingModel> = None;
    let mut notes = Vec::new();
    let venue = asked.venue.as_str();
    for symbol in asked.symbols.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let answer = match answer_for(venue, &asked.source, symbol).await {
            Ok(answer) => answer,
            Err(detail) => return fail(&detail),
        };
        let observed = match bijective_venue_registry::observe_instrument(venue, &answer, symbol) {
            Ok(observed) => observed,
            Err(e) => return fail(&format!("{symbol}: {e}")),
        };
        funding = funding.or_else(|| observed.funding.get(&Transport::Stream).cloned());
        let settles_in = asked.settles_in.as_deref();
        match instrument_from(venue, symbol, &observed, settles_in, &mut notes) {
            Ok(instrument) => instruments.push(instrument),
            Err(e) => return fail(&e.to_string()),
        }
    }
    let settlement = instruments.first().map(|i| i.settlement_currency.clone());
    let desk: Vec<String> = instruments.iter().map(|i| format!("\"{}\"", i.id)).collect();
    let profile = match draft(venue, instruments, funding) {
        Ok(profile) => profile,
        Err(e) => return fail(&e.to_string()),
    };
    for note in &notes {
        eprintln!("note: {note}");
    }
    eprintln!("drafted from the venue's published rules; unverified until bijective-verify-venue");
    let Some(out) = asked.out.as_deref() else {
        println!("{}", profile.to_json());
        return ExitCode::SUCCESS;
    };
    let settlement = settlement.unwrap_or_default();
    match write_out(out, &profile.to_json(), &asked, (&settlement, &desk)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(detail) => fail(&detail),
    }
}

/// The profile and a starter configuration that names it by its absolute path.
fn write_out(
    out: &std::path::Path,
    profile: &str,
    asked: &Asked,
    (settlement, desk): (&str, &[String]),
) -> Result<(), String> {
    std::fs::create_dir_all(out)
        .map_err(|e| format!("{} could not be made: {e}", out.display()))?;
    let profile_path = out.join("profile.json");
    std::fs::write(&profile_path, format!("{profile}\n"))
        .map_err(|e| format!("{} could not be written: {e}", profile_path.display()))?;
    let profile_path = profile_path.canonicalize().unwrap_or(profile_path);
    let config_path = out.join("config.json");
    let body = starter_config(asked, &profile_path.display().to_string(), settlement, desk);
    std::fs::write(&config_path, body)
        .map_err(|e| format!("{} could not be written: {e}", config_path.display()))?;
    println!("profile:  {}", profile_path.display());
    println!(
        "config:   {} (a starter for paper and backtest: set its order rate, limits and sandbox \
         balance before live)",
        config_path.display()
    );
    Ok(())
}

/// One desk over every drafted instrument, a sandbox of 10,000 in the settlement currency, a
/// portfolio gross exposure limit at the same size and an order cap of a tenth of it, all at the
/// money scale. Credential names are placeholders: paper and backtest read no credential.
fn starter_config(asked: &Asked, profile: &str, settle: &str, ids: &[String]) -> String {
    let venue = &asked.venue;
    format!(
        r#"{{
  "version": 1,
  "venues": ["{venue}"],
  "venue_access": [
    {{
      "name": "{venue}",
      "endpoint": "{endpoint}",
      "profile": "{profile}",
      "key_name": "{venue}-key",
      "secret_name": "{venue}-secret",
      "wallet_key_name": "{venue}-wallet-key",
      "passphrase_name": "{venue}-passphrase",
      "account_address": "set-before-live",
      "settle_coin": "{settle}",
      "request_timeout_ns": "10000000000",
      "order_rate": {{ "orders": 5, "per_ns": "1000000000" }}
    }}
  ],
  "desks": [
    {{
      "name": "alpha",
      "instruments": [{ids}],
      "venues": ["{venue}"],
      "maximum_per_order_scaled": "100000000000"
    }}
  ],
  "limits": [
    {{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "1000000000000" }}
  ],
  "sandbox": {{ "{venue}": {{ "{settle}": "1000000000000" }} }},
  "freshness_bound_ns": "5000000000"
}}
"#,
        endpoint = asked.source,
        ids = ids.join(", "),
    )
}

/// The venue's instruments answer for `symbol`, fetched from a URL or read from a recorded file.
async fn answer_for(venue: &str, source: &str, symbol: &str) -> Result<String, String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        return bijective_venue_registry::fetch_instrument_answer(venue, source, symbol)
            .await
            .map_err(|e| format!("the venue could not be reached: {e}"));
    }
    std::fs::read_to_string(source).map_err(|e| format!("{source} could not be read: {e}"))
}

fn fail(detail: &str) -> ExitCode {
    eprintln!("bijective-profile: {detail}");
    ExitCode::FAILURE
}
