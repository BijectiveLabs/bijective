// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Gates for venue assurance's acceptance criteria 1 to 5. Criteria 6 to 8 belong to V5 and V6.

use std::collections::BTreeMap;

use super::*;
use crate::profile::declaration::{PROFILE_FORMAT, TransportProfile, VerificationStamp};
use crate::profile::instrument::{
    Instrument, InstrumentCatalogue, InstrumentKind, InstrumentStatus,
};
use bijective_model::{InstrumentId, VenueSymbol};

fn profile() -> VenueProfile {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Published { interval_ns: 28_800_000_000_000 },
            data_classes: vec![DataClass::TopOfBook, DataClass::Trades, DataClass::Funding],
            max_staleness_ns: Some(5_000_000_000),
        },
    );
    transports.insert(
        Transport::Request,
        TransportProfile {
            funding: FundingModel::UnpublishedOnThisTransport,
            data_classes: vec![DataClass::AccountState],
            max_staleness_ns: Some(30_000_000_000),
        },
    );

    VenueProfile {
        format: PROFILE_FORMAT,
        venue: "venue-a".into(),
        verified_at: Some(VerificationStamp {
            at_ns: 1_700_000_000_000_000_000,
            verifier_version: "1".into(),
        }),
        transports,
        reduce_only: ReduceOnly::FillTimeClamp,
        order_types: vec![
            OrderTypeSupport { name: "limit".into(), native: true },
            OrderTypeSupport { name: "trailing-stop".into(), native: false },
        ],
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
                price_tick: 5,
                quantity_lot: 10,
                price_validity: PriceValidity::IncrementOnly,
            },
            minimum_size: 1,
            minimum_notional: 1,
            status: InstrumentStatus::Trading,
        }]),
        identifier_rules: IdentifierRules {
            allowed: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".into(),
            max_len: 36,
        },
        transformations: vec![Transformation::PriceSignificantFigures {
            field: "price".into(),
            figures: 5,
        }],
        support: SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared"),
    }
}

/// Criterion 1 - a profile round-trips through the open format.
#[test]
fn a_profile_round_trips_as_json() {
    let p = profile();
    let text = p.to_json();
    assert!(text.contains("\"venue\""), "not readable without our tooling");
    assert_eq!(VenueProfile::from_json(&text).unwrap(), p);
}

#[test]
fn a_newer_profile_format_is_refused() {
    let mut p = profile();
    p.format = PROFILE_FORMAT + 1;
    let err = VenueProfile::from_json(&p.to_json()).unwrap_err();
    assert!(matches!(err, ProfileError::UnsupportedFormat { .. }));
}

#[test]
fn a_profile_with_no_transport_is_refused() {
    let mut p = profile();
    p.transports.clear();
    assert!(matches!(VenueProfile::from_json(&p.to_json()), Err(ProfileError::NoTransports)));
}

/// Criterion 2 - the declaration is derived, and editing the profile changes it.
#[test]
fn the_declaration_follows_the_profile() {
    let mut p = profile();
    let before = CapabilityDeclaration::from_profile(&p);
    assert!(before.supports_natively("limit"));

    p.order_types[0].native = false;
    let after = CapabilityDeclaration::from_profile(&p);
    assert!(!after.supports_natively("limit"), "the declaration did not follow");
}

#[test]
fn an_emulated_order_type_reads_as_unsupported() {
    let d = CapabilityDeclaration::from_profile(&profile());
    assert!(d.supports_natively("limit"));
    assert!(!d.supports_natively("trailing-stop"));
}

#[test]
fn an_unverified_profile_says_so() {
    let mut p = profile();
    p.verified_at = None;
    assert!(!CapabilityDeclaration::from_profile(&p).is_verified());
    assert!(CapabilityDeclaration::from_profile(&profile()).is_verified());
}

#[test]
fn staleness_is_the_tightest_bound_across_transports() {
    let d = CapabilityDeclaration::from_profile(&profile());
    assert_eq!(d.max_staleness_ns(), Some(5_000_000_000));
}

/// Criterion 3 - all five models are representable, and absence is not.
#[test]
fn every_funding_model_is_representable_and_absence_is_not() {
    let models = [
        FundingModel::Continuous,
        FundingModel::Published { interval_ns: 1 },
        FundingModel::UnpublishedOnThisTransport,
        FundingModel::FixedAndKnown { interval_ns: 1 },
        FundingModel::AdapterComputed { interval_ns: 1 },
    ];
    for m in models {
        let text = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<FundingModel>(&text).unwrap(), m);
    }
    // Both yield no interval and mean opposite things. Callers match on the model.
    assert_eq!(FundingModel::Continuous.interval_ns(), None);
    assert_eq!(FundingModel::UnpublishedOnThisTransport.interval_ns(), None);
    assert_ne!(FundingModel::Continuous, FundingModel::UnpublishedOnThisTransport);
}

#[test]
fn an_adapter_computed_interval_is_labelled_derived() {
    assert!(FundingModel::AdapterComputed { interval_ns: 1 }.is_derived());
    assert!(!FundingModel::Published { interval_ns: 1 }.is_derived());
    assert!(!FundingModel::FixedAndKnown { interval_ns: 1 }.is_derived());
}

/// Criterion 4 - a figure carries its transport, and the model is not the payment.
#[test]
fn a_funding_figure_carries_its_transport() {
    let f = FundingFigure::new(125, Transport::Stream, FundingModel::Continuous);
    assert_eq!(f.transport(), Transport::Stream);
    assert!(!f.implies_payment(), "a rate is reference data, not a payment");
}

/// Criterion 5 - a rejection constraint is enforceable, not advisory.
#[test]
fn an_identifier_the_venue_would_reject_is_refused_here_first() {
    let p = profile();
    let with_hyphens = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";
    assert!(matches!(
        p.identifier_rules.check(with_hyphens),
        Err(IdentifierRejection::Character { found: '-', .. })
    ));
    assert!(p.identifier_rules.check("3f2504e04f8911d39a0c0305e82c3301").is_ok());
}

#[test]
fn an_over_long_or_empty_identifier_is_refused() {
    let p = profile();
    assert!(matches!(
        p.identifier_rules.check(&"a".repeat(37)),
        Err(IdentifierRejection::TooLong { max: 36, .. })
    ));
    assert!(matches!(p.identifier_rules.check(""), Err(IdentifierRejection::Empty)));
}

#[test]
fn a_transformed_field_is_named_in_the_declaration() {
    let d = CapabilityDeclaration::from_profile(&profile());
    assert_eq!(d.altered_fields(), ["price"]);
}

/// Risk limits selects Reducing only where the venue refuses at placement. A fill-time clamp is
/// declared as weaker.
#[test]
fn a_fill_time_clamp_is_declared_rather_than_flattened() {
    assert_eq!(
        CapabilityDeclaration::from_profile(&profile()).reduce_only(),
        ReduceOnly::FillTimeClamp
    );
}

/// Asserts the string form: a JSON number is a double in most parsers and an operator's tooling
/// would round it.
#[test]
fn wide_integers_are_written_as_strings_so_a_clients_tooling_cannot_round_them() {
    let p = profile();
    let text = p.to_json();

    let stamp = p.verified_at.as_ref().unwrap().at_ns;
    assert_eq!(stamp, 1_700_000_000_000_000_000);
    assert!(
        text.contains(&format!("\"{stamp}\"")),
        "the timestamp was written as a bare number, which a double cannot hold exactly"
    );
    assert!(!text.contains(&format!(": {stamp}")), "a bare numeric form is present somewhere");

    assert_eq!(VenueProfile::from_json(&text).unwrap().verified_at.unwrap().at_ns, stamp);
}

#[test]
fn a_nanosecond_timestamp_does_not_survive_a_double() {
    let exact: i128 = 1_700_000_000_123_456_789;
    let through_double = exact as f64 as i128;
    assert_ne!(exact, through_double, "if these were equal the string form would be unnecessary");
}

#[test]
fn a_venue_we_cannot_debug_cannot_be_declared() {
    assert!(matches!(
        SupportEntry::declare("venue-x", SupportDepth::Full, false),
        Err(SupportError::NotDebuggable { .. })
    ));
    assert!(SupportEntry::declare("venue-x", SupportDepth::Full, true).is_ok());
}

/// A price off the venue's tick is refused here, not discovered at placement.
#[test]
fn a_price_off_the_tick_is_refused() {
    let p = profile();
    assert!(
        p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_price(2_654_375).is_ok()
    );
    assert!(matches!(
        p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_price(2_654_376),
        Err(IncrementRejection::OffIncrement { increment: 5, .. })
    ));
}

#[test]
fn a_quantity_off_the_lot_is_refused() {
    let p = profile();
    assert!(p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_quantity(120).is_ok());
    assert!(matches!(
        p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_quantity(125),
        Err(IncrementRejection::OffIncrement { increment: 10, .. })
    ));
}

/// An increment of zero is not an increment. Refused, not read as no constraint.
#[test]
fn an_undeclared_increment_is_refused_rather_than_ignored() {
    let mut p = profile();
    let sym = VenueSymbol::new("aaa");
    let mut fixed = p.instruments.get(&sym).unwrap().clone();
    fixed.precision.price_tick = 0;
    p.instruments.record(fixed);
    assert!(matches!(
        p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_price(100),
        Err(IncrementRejection::IncrementNotDeclared)
    ));
}

#[test]
fn the_increment_check_holds_for_negatives() {
    let p = profile();
    assert!(
        p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_price(-2_654_375).is_ok()
    );
    assert!(
        p.instruments.precision(&VenueSymbol::new("aaa")).unwrap().check_price(-2_654_376).is_err()
    );
}

/// The catalogue's own tests never go through JSON. A node once refused a hand-written profile
/// at start-up.
#[test]
fn a_profile_with_instruments_round_trips_through_the_open_format() {
    let original = profile();
    let text = serde_json::to_string(&original).expect("serialise");

    assert!(
        text.contains("\"instruments\":[{"),
        "a profile declares a list of instruments, not an object wrapping one: {text}"
    );

    let read = VenueProfile::from_json(&text).expect("parse");
    assert_eq!(read.instruments, original.instruments);
    assert_eq!(
        read.instruments.precision(&VenueSymbol::new("aaa")).expect("known").price_tick,
        original.instruments.precision(&VenueSymbol::new("aaa")).expect("known").price_tick
    );
}

/// Catalogue enums follow the format's convention, so an operator does not switch conventions
/// mid-file.
#[test]
fn instrument_kinds_and_statuses_are_written_the_way_the_rest_of_the_profile_is() {
    let text = serde_json::to_string(&profile()).expect("serialise");

    assert!(text.contains("\"perpetual\""), "{text}");
    assert!(text.contains("\"trading\""), "{text}");
}

/// The venue's own worked examples at price scale 2, from its documentation: 1234.5 is valid,
/// 1234.56 is not, and integer prices are always allowed.
#[test]
fn the_venues_own_price_examples_decide_the_same_way_the_venue_does() {
    let rule = PriceValidity::SignificantFigures { at_most: 5 };

    assert!(rule.admits(123_450, 2), "1234.5 - five significant figures");
    assert!(!rule.admits(123_456, 2), "1234.56 - six, and not an integer");

    assert!(
        rule.admits(12_345_600, 2),
        "123456 - an integer price, and integers are always allowed"
    );
    assert!(!rule.admits(1_234_560, 2), "12345.6 - six significant figures and not an integer");
}

#[test]
fn trailing_zeros_do_not_count_against_the_cap() {
    let rule = PriceValidity::SignificantFigures { at_most: 5 };

    // 12.30 at scale 2 is 1230: two trailing zeros, three significant figures.
    assert!(rule.admits(1_230, 2));
    assert!(rule.admits(0, 2), "no price is not too precise");
}

#[test]
fn a_venue_with_no_significant_figure_rule_constrains_nothing_further() {
    assert!(PriceValidity::IncrementOnly.admits(123_456_789, 2));
}

#[test]
fn the_refusal_names_the_cap() {
    let rules = PrecisionRules {
        price_scale: 2,
        quantity_scale: 3,
        price_tick: 1,
        quantity_lot: 1,
        price_validity: PriceValidity::SignificantFigures { at_most: 5 },
    };

    let said = rules.check_price(123_456).expect_err("six figures").to_string();
    assert!(said.contains('5'), "{said}");
    assert!(said.contains("integer"), "the exception is part of the rule: {said}");
}

/// The second path for the hard rule is an operator's profile: a derived `Deserialize` asks nothing.
#[test]
fn a_venue_we_cannot_debug_cannot_arrive_through_a_profile_either() {
    let wire = r#"{"venue":"ghost","depth":"full","debuggable_to_root_cause":false}"#;
    let parsed: Result<SupportEntry, _> = serde_json::from_str(wire);
    assert!(parsed.is_err(), "deserialisation walked past the rule declare() enforces: {parsed:?}");
}

#[test]
fn a_debuggable_venue_still_round_trips() {
    let entry = SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared");
    let text = serde_json::to_string(&entry).expect("serialise");
    assert_eq!(serde_json::from_str::<SupportEntry>(&text).expect("parse"), entry);
}

/// A curation belongs to its venue: a data-only venue must not get a full-support entry copied
/// from another.
#[test]
fn a_profile_cannot_carry_another_venues_support_entry() {
    let mut p = profile();
    p.support = SupportEntry::declare("venue-b", SupportDepth::Full, true).expect("declared");

    match VenueProfile::from_json(&p.to_json()) {
        Err(ProfileError::SupportNamesAnotherVenue { profile, support }) => {
            assert_eq!(profile, p.venue);
            assert_eq!(support, "venue-b");
        }
        other => panic!("a borrowed curation parsed: {other:?}"),
    }
}

/// The classes the market contract added are written the way an operator writes the others, so
/// a profile can declare everything an adapter emits.
#[test]
fn every_data_class_has_a_written_name_that_reads_back() {
    let classes = [
        (DataClass::Depth, "\"depth\""),
        (DataClass::OpenInterest, "\"open-interest\""),
        (DataClass::Liquidations, "\"liquidations\""),
        (DataClass::IndexPrice, "\"index-price\""),
        (DataClass::LongShortRatio, "\"long-short-ratio\""),
    ];
    for (class, written) in classes {
        assert_eq!(serde_json::to_string(&class).expect("serialise"), written);
        assert_eq!(serde_json::from_str::<DataClass>(written).expect("parse"), class);
    }
}
