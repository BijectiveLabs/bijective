// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use bijective_venue::profile::SupportDepth;
use std::collections::BTreeMap;

fn pair(price: u8, quantity: u8) -> Scales {
    let scale = |digits| bijective_venue::normalise::Scale::new(digits).unwrap();
    Scales { price: scale(price), quantity: scale(quantity) }
}

fn scales() -> InstrumentScales {
    InstrumentScales::uniform_over(["BTCUSDT", "ETHUSDT"], pair(2, 3))
}

/// Bybit reads each instrument at its own scales. A single-scale feed refuses mixed precision
/// rather than reading one instrument at the other's (decision 0007).
#[test]
fn mixed_precision_opens_where_scales_are_per_instrument_and_is_refused_elsewhere() {
    let mixed: InstrumentScales = [
        (bijective_model::VenueSymbol::new("BTCUSDT"), pair(2, 3)),
        (bijective_model::VenueSymbol::new("ETHUSDT"), pair(2, 2)),
    ]
    .into_iter()
    .collect();
    assert!(open_market_feed("bybit", &mixed).is_ok());
    match open_market_feed("hyperliquid", &mixed) {
        Err(ConnectError::NotPermitted { detail }) => {
            assert!(detail.contains("BTCUSDT"), "{detail}")
        }
        Err(other) => panic!("refused for the wrong reason: {other}"),
        Ok(_) => panic!("a single-scale feed took two precisions"),
    }
}

#[test]
fn a_registered_venue_opens() {
    let s = scales();
    assert!(open_market_feed("bybit", &s).is_ok());
}

/// One venue, one spelling: a folded name became two identities for the feed and its limits.
#[test]
fn a_name_that_differs_only_in_case_is_refused_with_the_list() {
    let s = scales();
    match open_market_feed("ByBit", &s) {
        Err(ConnectError::NotPermitted { detail }) => {
            assert!(detail.contains("ByBit"), "the refusal does not name what was given: {detail}");
            assert!(detail.contains("bybit"), "the refusal does not offer the spelling: {detail}");
        }
        Err(other) => panic!("refused for the wrong reason: {other}"),
        Ok(_) => panic!("two spellings of one venue were both accepted"),
    }
}

/// No default. A node whose venue name is wrong must fail to open, not connect somewhere else.
#[test]
fn an_unregistered_venue_is_refused_rather_than_defaulted() {
    let s = scales();
    match open_market_feed("not-a-venue", &s) {
        Err(ConnectError::NotPermitted { detail }) => assert!(detail.contains("not-a-venue")),
        Err(other) => panic!("refused for the wrong reason: {other}"),
        Ok(_) => panic!("an unregistered venue was opened"),
    }
}

#[test]
fn an_empty_name_is_refused() {
    let s = scales();
    assert!(open_market_feed("", &s).is_err());
}

/// A venue curated for data has no execution to open. The executor is built from the
/// declaration, so the check cannot be forgotten.
#[test]
fn a_venue_curated_for_data_only_cannot_open_an_executor() {
    let refusal = open_execution(REGISTERED[0], setup_declaring(SupportDepth::DataOnly));
    match refusal {
        Err(ConnectError::NotPermitted { detail }) => {
            assert!(detail.contains("market data only"), "{detail}");
        }
        Ok(_) => panic!("a data-only venue opened an executor"),
        Err(e) => panic!("refused, but not for the curated depth: {e:?}"),
    }
}

#[test]
fn a_venue_curated_in_full_still_opens() {
    assert!(open_execution(REGISTERED[0], setup_declaring(SupportDepth::Full)).is_ok());
}

fn setup_declaring(depth: SupportDepth) -> ExecutionSetup<'static> {
    ExecutionSetup {
        endpoint: "http://127.0.0.1:9",
        credential: VenueCredential::KeyAndSecret {
            api_key: "unused".to_string(),
            secret: Credential::new(b"unused".to_vec()),
        },
        declaration: declaration(depth),
        timeout: Duration::from_secs(5),
        clock: Arc::new(bijective_common::clock::WallClock::new()),
        settle_coin: "USDT",
    }
}

fn declaration(depth: SupportDepth) -> CapabilityDeclaration {
    use bijective_venue::profile::{
        CapabilityDeclaration, DataClass, IdentifierRules, Instrument, InstrumentCatalogue,
        InstrumentId, InstrumentKind, InstrumentStatus, OrderTypeSupport, PrecisionRules,
        PriceValidity, ReduceOnly, SupportEntry, Transport, TransportProfile, VenueProfile,
        VenueSymbol, declaration::PROFILE_FORMAT,
    };

    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: bijective_venue::profile::FundingModel::Published {
                interval_ns: 28_800_000_000_000,
            },
            data_classes: vec![DataClass::TopOfBook],
            max_staleness_ns: Some(5_000_000_000),
        },
    );

    CapabilityDeclaration::from_profile(&VenueProfile {
        format: PROFILE_FORMAT,
        venue: REGISTERED[0].into(),
        verified_at: None,
        transports,
        reduce_only: ReduceOnly::PlacementPrecondition,
        order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
        instruments: InstrumentCatalogue::from_instruments([Instrument {
            symbol: VenueSymbol::new("aaa"),
            id: InstrumentId::new("aaa"),
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
        transformations: vec![],
        support: SupportEntry::declare(REGISTERED[0], depth, true).expect("declared"),
    })
}

/// A wrong-venue observation verifies the wrong declaration, so unregistered names are refused.
#[test]
fn an_unregistered_venue_has_no_observer() {
    let answer = r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[]}}"#;

    match observe_instrument("not-a-venue", answer, "BTCUSDT") {
        Err(ConnectError::NotPermitted { detail }) => {
            assert!(detail.contains("not a registered venue"), "{detail}");
        }
        other => panic!("an unregistered venue must have no observer: {other:?}"),
    }
}

/// Every registered venue has a reader now. Each reads its own answer and refuses another's.
#[test]
fn every_registered_venue_reads_its_own_answer_and_refuses_another() {
    let meta = r#"{"universe":[{"szDecimals":5,"name":"BTC","maxLeverage":40}]}"#;
    let observed = observe_instrument("hyperliquid", meta, "BTC").expect("read");
    assert_eq!(observed.currencies.expect("observed").base, "BTC");

    match observe_instrument("hyperliquid", r#"{"retCode":0,"retMsg":"OK"}"#, "BTC") {
        Err(ConnectError::NotPermitted { detail }) => {
            assert!(!detail.contains("not a registered venue"), "it is registered: {detail}");
        }
        other => panic!("another venue's answer must be refused: {other:?}"),
    }
}

#[test]
fn a_registered_venue_reads_its_own_answer() {
    let answer = r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[
{"symbol":"BTCUSDT","status":"Trading","baseCoin":"BTC","quoteCoin":"USDT","settleCoin":"USDT","priceScale":"2",
 "lotSizeFilter":{"maxOrderQty":"1500.000","minOrderQty":"0.001","qtyStep":"0.001","minNotionalValue":"5"},
 "priceFilter":{"minPrice":"0.10","maxPrice":"1999999.80","tickSize":"0.10"},
 "fundingInterval":480}]}}"#;

    let observed = observe_instrument("bybit", answer, "BTCUSDT").expect("read");

    assert_eq!(observed.precision.expect("published").rules.price_tick, 10);
}

#[test]
fn every_registered_venue_names_its_credential_shape() {
    for venue in REGISTERED {
        assert!(credential_shape(venue).is_ok(), "{venue}");
    }
    assert!(credential_shape("Bybit").is_err());
}
