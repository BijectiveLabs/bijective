// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Runs the venue conformance suite and the profile verifier against the real adapter.
//! Usage: conformance_probe <stream-endpoint> <instrument> [request-endpoint]

use bijective_venue::profile::{SupportDepth, SupportEntry};
use std::collections::BTreeMap;

use bijective_common::clock::WallClock;
use bijective_venue::contract::StreamRequest;
use bijective_venue::contract::conformance::{
    CheckOutcome, DeclarationCheck, check_data, is_releasable,
};
use bijective_venue::normalise::Scale;
use bijective_venue::profile::{
    CapabilityDeclaration, DataClass, FundingModel, IdentifierRules, Instrument,
    InstrumentCatalogue, InstrumentId, InstrumentKind, InstrumentStatus, OrderTypeSupport,
    PrecisionRules, PriceValidity, ReduceOnly, Transport, TransportProfile, VenueProfile,
    VenueSymbol, declaration::PROFILE_FORMAT,
};
use bijective_venue_assurance::{AssuranceReport, NotObservedReason, Verification, verify};
use bijective_venue_bybit::market::MarketData;
use bijective_venue_bybit::observe::observe_instrument;

/// Deliberately unverified: a verified profile comes from the verifier, not the adapter.
fn drill_profile(symbol: &str) -> VenueProfile {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Published { interval_ns: 28_800_000_000_000 },
            data_classes: vec![DataClass::TopOfBook, DataClass::Depth, DataClass::Trades],
            max_staleness_ns: Some(40_000_000_000),
        },
    );

    VenueProfile {
        format: PROFILE_FORMAT,
        venue: "the venue under drill".into(),
        verified_at: None,
        transports,
        reduce_only: ReduceOnly::FillTimeClamp,
        order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
        instruments: InstrumentCatalogue::from_instruments([Instrument {
            symbol: VenueSymbol::new(symbol),
            id: InstrumentId::new(symbol),
            kind: InstrumentKind::Perpetual,
            settlement_currency: "usdt".into(),
            quote_currency: "usdt".into(),
            base_currency: None,
            precision: PrecisionRules {
                price_scale: 2,
                quantity_scale: 3,
                price_tick: 10,
                quantity_lot: 1,
                price_validity: PriceValidity::IncrementOnly,
            },
            minimum_size: 1,
            minimum_notional: 500,
            status: InstrumentStatus::Trading,
        }]),
        identifier_rules: IdentifierRules {
            allowed: "abcdefghijklmnopqrstuvwxyz0123456789-".into(),
            max_len: 36,
        },
        transformations: Vec::new(),
        support: SupportEntry::declare("bybit", SupportDepth::Full, true).expect("declared"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().ok_or("usage: conformance_probe <endpoint> <instrument>")?;
    let instrument = args.next().ok_or("an instrument is required")?;

    let declaration = CapabilityDeclaration::from_profile(&drill_profile(&instrument));
    let request = StreamRequest {
        endpoint,
        instruments: vec![instrument.clone()],
        scales: bijective_venue::profile::InstrumentScales::uniform_over(
            [instrument.as_str()],
            bijective_venue::profile::Scales { price: Scale::new(2)?, quantity: Scale::new(3)? },
        ),
        clock: std::sync::Arc::new(WallClock::new()),
    };
    let mut adapter = MarketData::new(request, declaration);

    // Verifier first so its answer feeds the suite declaration check.
    let rest = std::env::args().nth(3).unwrap_or_else(|| "https://api.bybit.com".to_string());
    let declaration = assure(&rest, &instrument).await;
    println!();

    let now_ns = 0;
    let results =
        check_data(&mut adapter, &VenueSymbol::new(instrument.clone()), now_ns, &declaration).await;

    let failed = print_results(&results);

    adapter_disconnect(&mut adapter).await;

    println!();
    println!(
        "{} check(s), {failed} failed, releasable: {}",
        results.len(),
        is_releasable(&results)
    );
    if failed > 0 {
        return Err("the adapter did not pass its own contract".into());
    }
    Ok(())
}

async fn adapter_disconnect(adapter: &mut MarketData) {
    use bijective_venue::contract::VenueData;
    adapter.disconnect().await;
}

/// The verifier's answer, which feeds the suite's declaration check. No answer is not a confirmation:
/// a venue that could not be observed leaves the declaration unverified.
async fn assure(rest: &str, instrument: &str) -> DeclarationCheck {
    match observe_instrument(rest, instrument).await {
        Ok(observed) => match verify(&drill_profile(instrument), &observed, 0) {
            Verification::Confirmed { .. } => {
                println!("ASSURANCE    the venue behaves as the profile declares");
                DeclarationCheck::Matches
            }
            Verification::Diverged { findings, .. } => {
                println!(
                    "ASSURANCE    the profile and the venue disagree, {} finding(s):",
                    findings.len()
                );
                for f in &findings {
                    let weight =
                        if f.is_capital_relevant() { "capital-relevant" } else { "reporting" };
                    println!("             [{weight}] {f}");
                }
                DeclarationCheck::Diverges { detail: format!("{} finding(s)", findings.len()) }
            }
            // Unreachable for an observation. Kept because the enum has three variants.
            Verification::NotObserved { reason, .. } => {
                println!("ASSURANCE    not observed: {reason:?}");
                DeclarationCheck::Unverified
            }
        },
        Err(e) => {
            // No answer is not a confirmation: record the failed check as NotObserved.
            let unproven = AssuranceReport::new(
                "bybit",
                Verification::NotObserved { at_ns: 0, reason: NotObservedReason::VenueUnreachable },
            );
            println!("ASSURANCE    could not observe the venue: {e}");
            if unproven.raises_condition() {
                println!("             the declaration is unproven, which is not disproven");
            }
            DeclarationCheck::Unverified
        }
    }
}

/// One line per check, with the failure detail. Returns the number that failed.
fn print_results(results: &[bijective_venue::contract::conformance::CheckResult]) -> usize {
    let mut failed = 0;
    for r in results {
        let label = match r.outcome {
            CheckOutcome::Passed => "PASS",
            CheckOutcome::Failed { .. } => {
                failed += 1;
                "FAIL"
            }
            // Unproven until a live venue is present.
            CheckOutcome::RequiresLiveVenue => "LIVE-NEEDED",
        };
        println!("{label:<12} {}", r.name);
        if let CheckOutcome::Failed { detail } = &r.outcome {
            println!("             {detail}");
        }
    }
    failed
}
