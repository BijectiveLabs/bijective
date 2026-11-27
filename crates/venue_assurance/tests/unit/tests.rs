// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Venue assurance acceptance criterion 6, and the divergence classes.

use bijective_venue::profile::{SupportDepth, SupportEntry};
use std::collections::BTreeMap;

use bijective_venue::profile::{
    DataClass, IdentifierRules, Instrument, InstrumentCatalogue, InstrumentId, InstrumentKind,
    InstrumentStatus, ObservedPrecision, OrderTypeSupport, PrecisionRules, PriceValidity,
    ReduceOnly, Transport, TransportProfile, VenueProfile, VenueSymbol, VerificationStamp,
    declaration::PROFILE_FORMAT,
};

use super::*;

fn profile() -> VenueProfile {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Published { interval_ns: 28_800_000_000_000 },
            data_classes: vec![DataClass::TopOfBook],
            max_staleness_ns: Some(5_000_000_000),
        },
    );
    VenueProfile {
        format: PROFILE_FORMAT,
        venue: "venue-a".into(),
        verified_at: Some(VerificationStamp { at_ns: 1, verifier_version: "1".into() }),
        transports,
        reduce_only: ReduceOnly::PlacementPrecondition,
        order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
        instruments: InstrumentCatalogue::from_instruments([Instrument {
            symbol: VenueSymbol::new("aaa"),
            id: InstrumentId::new("aaa"),
            kind: InstrumentKind::Perpetual,
            settlement_currency: "usdt".into(),
            quote_currency: "usdt".into(),
            base_currency: Some("aaa-base".into()),
            precision: PrecisionRules {
                price_scale: 2,
                quantity_scale: 3,
                price_tick: 5,
                quantity_lot: 10,
                price_validity: PriceValidity::IncrementOnly,
            },
            minimum_size: 1,
            minimum_notional: 1,
            status: InstrumentStatus::Trading,
        }]),
        identifier_rules: IdentifierRules { allowed: "abc123".into(), max_len: 8 },
        transformations: vec![],
        support: SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared"),
    }
}

#[test]
fn a_matching_venue_confirms() {
    let p = profile();
    let mut o = ObservedBehaviour {
        reduce_only: Some(ReduceOnly::PlacementPrecondition),
        native_order_types: Some(vec!["limit".into()]),
        precision: Some(ObservedPrecision {
            symbol: VenueSymbol::new("aaa"),
            rules: p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().clone(),
        }),
        ..Default::default()
    };
    o.funding
        .insert(Transport::Stream, FundingModel::Published { interval_ns: 28_800_000_000_000 });
    assert!(verify(&p, &o, 100).is_confirmed());
}

/// **Criterion 6.** A failed verification leaves the profile byte-identical.
#[test]
fn a_failed_verification_does_not_touch_the_profile() {
    let p = profile();
    let before = p.to_json();

    let o =
        ObservedBehaviour { reduce_only: Some(ReduceOnly::FillTimeClamp), ..Default::default() };
    let v = verify(&p, &o, 200);

    assert!(!v.is_confirmed());
    assert_eq!(p.to_json(), before, "the check rewrote what it was checking");
}

/// The severe direction: the profile claims a guarantee the venue does not make.
#[test]
fn a_reduce_only_downgrade_is_capital_relevant() {
    let p = profile();
    let o =
        ObservedBehaviour { reduce_only: Some(ReduceOnly::FillTimeClamp), ..Default::default() };
    match verify(&p, &o, 300) {
        Verification::Diverged { findings, .. } => {
            assert_eq!(findings.len(), 1);
            assert!(findings[0].is_capital_relevant());
            assert!(matches!(findings[0], Divergence::ReduceOnly { .. }));
        }
        other => panic!("expected divergence, got {other:?}"),
    }
}

#[test]
fn an_order_type_the_venue_does_not_support_natively_is_found() {
    let p = profile();
    let o = ObservedBehaviour { native_order_types: Some(vec![]), ..Default::default() };
    match verify(&p, &o, 400) {
        Verification::Diverged { findings, .. } => {
            assert!(matches!(findings[0], Divergence::OrderTypeNotNative { .. }));
            assert!(findings[0].is_capital_relevant());
        }
        other => panic!("expected divergence, got {other:?}"),
    }
}

#[test]
fn an_undeclared_native_type_is_found_and_is_not_capital_relevant() {
    let p = profile();
    let o = ObservedBehaviour {
        native_order_types: Some(vec!["limit".into(), "post-only".into()]),
        ..Default::default()
    };
    match verify(&p, &o, 500) {
        Verification::Diverged { findings, .. } => {
            assert_eq!(findings.len(), 1);
            assert!(matches!(findings[0], Divergence::OrderTypeNativeUndeclared { .. }));
            assert!(!findings[0].is_capital_relevant());
        }
        other => panic!("expected divergence, got {other:?}"),
    }
}

#[test]
fn a_funding_model_change_is_found_per_transport() {
    let p = profile();
    let mut o = ObservedBehaviour::default();
    o.funding.insert(Transport::Stream, FundingModel::Continuous);
    match verify(&p, &o, 600) {
        Verification::Diverged { findings, .. } => {
            assert!(matches!(
                findings[0],
                Divergence::Funding { transport: Transport::Stream, .. }
            ));
        }
        other => panic!("expected divergence, got {other:?}"),
    }
}

#[test]
fn an_unobserved_field_is_not_a_finding() {
    assert!(verify(&profile(), &ObservedBehaviour::default(), 700).is_confirmed());
}

#[test]
fn an_unreachable_venue_is_not_a_pass() {
    let v = Verification::NotObserved { at_ns: 800, reason: NotObservedReason::VenueUnreachable };
    let r = AssuranceReport::new("venue-a", v);
    assert!(!r.verification.is_confirmed());
    assert!(r.raises_condition());
    assert_eq!(r.unverified_as_of(), Some(800));
}

#[test]
fn a_missing_retrieval_method_is_a_profile_defect() {
    let v =
        Verification::NotObserved { at_ns: 900, reason: NotObservedReason::RetrievalMethodGone };
    assert!(matches!(
        v,
        Verification::NotObserved { reason: NotObservedReason::RetrievalMethodGone, .. }
    ));
    assert!(AssuranceReport::new("venue-a", v).raises_condition());
}

#[test]
fn a_confirmed_check_leaves_no_marker() {
    let r = AssuranceReport::new("venue-a", Verification::Confirmed { at_ns: 1000 });
    assert!(!r.raises_condition());
    assert_eq!(r.unverified_as_of(), None);
}

// the floors the venue enforces

/// A profile declaring a floor below the venue's own is a divergence, and a capital-relevant one.
#[test]
fn a_minimum_below_the_venues_own_is_a_capital_relevant_divergence() {
    let p = profile();
    let observed = ObservedBehaviour {
        minimums: Some(bijective_venue::profile::ObservedMinimums {
            symbol: bijective_venue::profile::VenueSymbol::new("aaa"),
            minimum_size: 1,
            minimum_notional: 500,
        }),
        ..Default::default()
    };

    match verify(&p, &observed, 1) {
        Verification::Diverged { findings, .. } => {
            let found = findings
                .iter()
                .find(|f| matches!(f, Divergence::Minimums { .. }))
                .expect("a minimum divergence");
            assert!(found.is_capital_relevant());
        }
        other => panic!("expected a divergence, got {other:?}"),
    }
}

#[test]
fn an_unpublished_floor_is_not_read_as_agreement() {
    let p = profile();
    let observed = ObservedBehaviour { minimums: None, ..Default::default() };

    assert!(matches!(verify(&p, &observed, 1), Verification::Confirmed { .. }));
}

#[test]
fn floors_that_match_the_venue_confirm() {
    let p = profile();
    let declared =
        p.instruments.get(&bijective_venue::profile::VenueSymbol::new("aaa")).expect("declared");
    let observed = ObservedBehaviour {
        minimums: Some(bijective_venue::profile::ObservedMinimums {
            symbol: bijective_venue::profile::VenueSymbol::new("aaa"),
            minimum_size: declared.minimum_size,
            minimum_notional: declared.minimum_notional,
        }),
        ..Default::default()
    };

    assert!(matches!(verify(&p, &observed, 1), Verification::Confirmed { .. }));
}

fn coins(base: &str, quote: &str, settlement: &str) -> ObservedBehaviour {
    ObservedBehaviour {
        currencies: Some(bijective_venue::profile::ObservedCurrencies {
            symbol: VenueSymbol::new("aaa"),
            base: base.into(),
            quote: Some(quote.into()),
            settlement: Some(settlement.into()),
        }),
        ..Default::default()
    }
}

/// A venue writes `USDT` where a profile writes `usdt`: one currency, so no finding.
#[test]
fn currencies_that_differ_only_in_case_confirm() {
    assert!(verify(&profile(), &coins("AAA-BASE", "USDT", "USDT"), 3).is_confirmed());
}

/// A settlement or base currency the venue does not use values and sums exposure wrongly.
#[test]
fn a_currency_the_venue_does_not_use_is_a_capital_relevant_divergence() {
    for observed in [coins("other", "usdt", "usdt"), coins("aaa-base", "usdt", "usdc")] {
        match verify(&profile(), &observed, 3) {
            Verification::Diverged { findings, .. } => {
                assert!(matches!(findings.as_slice(), [Divergence::Currencies { .. }]));
                assert!(findings[0].is_capital_relevant());
            }
            other => panic!("a currency mismatch confirmed: {other:?}"),
        }
    }
}

/// A profile that does not say what a quantity is counted in cannot be confirmed against a venue
/// that does.
#[test]
fn an_undeclared_base_currency_diverges() {
    let mut undeclared = profile();
    let instrument = undeclared.instruments.iter().next().expect("one").clone();
    undeclared.instruments =
        InstrumentCatalogue::from_instruments([Instrument { base_currency: None, ..instrument }]);

    assert!(!verify(&undeclared, &coins("aaa-base", "usdt", "usdt"), 3).is_confirmed());
}

/// A venue that does not state its quote or settlement currency is not disagreeing about it.
#[test]
fn an_unstated_currency_is_not_compared() {
    let mut observed = coins("aaa-base", "usdt", "usdt");
    if let Some(c) = observed.currencies.as_mut() {
        c.quote = None;
        c.settlement = None;
    }
    assert!(verify(&profile(), &observed, 3).is_confirmed());
}

/// 0.10 at scale 2 is 0.1 at scale 1: a profile at the platform's scales states the venue's rule.
#[test]
fn a_rule_at_another_scale_is_the_same_rule() {
    let observed = ObservedBehaviour {
        precision: Some(bijective_venue::profile::ObservedPrecision {
            symbol: VenueSymbol::new("aaa"),
            rules: PrecisionRules {
                price_scale: 3,
                quantity_scale: 2,
                price_tick: 50,
                quantity_lot: 1,
                price_validity: PriceValidity::IncrementOnly,
            },
        }),
        ..Default::default()
    };
    assert!(verify(&profile(), &observed, 3).is_confirmed());

    let finer = ObservedBehaviour {
        precision: observed.precision.clone().map(|mut p| {
            p.rules.price_tick = 5;
            p
        }),
        ..Default::default()
    };
    assert!(!verify(&profile(), &finer, 3).is_confirmed(), "a tick ten times finer confirmed");
}
