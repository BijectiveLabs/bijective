// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

/// Every refusal says what is wrong and why, and none of them says only *no*.
/// A validator answering `false` pushes the diagnosis back to the operator.
#[test]
fn every_reason_explains_itself() {
    let cases = [
        Invalid::new("store.path", Reason::Absent),
        Invalid::new("desk.maximum", Reason::NotPositive { found: 0 }),
        Invalid::new(
            "instrument.price_scale",
            Reason::OutOfRange { found: 40, minimum: 0, maximum: 18 },
        ),
        Invalid::new("desk.venues", Reason::Empty),
        Invalid::new("desk.venues", Reason::Duplicate { value: "venue-a".into() }),
        Invalid::new(
            "desk.venues",
            Reason::Unknown {
                value: "venue-z".into(),
                known: vec!["venue-a".into(), "venue-b".into()],
            },
        ),
        Invalid::new(
            "configuration",
            Reason::Malformed { detail: "expected `,` at line 4".into() },
        ),
    ];

    for case in &cases {
        let said = case.to_string();
        assert!(said.starts_with(&case.field), "a refusal names its field: {said}");
        assert!(
            said.len() > case.field.len() + 8,
            "a refusal that says only *no* is the failure this component exists to remove: {said}"
        );
    }
}

/// An unknown name carries what the platform does know.
#[test]
fn an_unknown_name_lists_the_ones_that_are_known() {
    let outcome = known("desk.venues", "venue-z", &["venue-b".to_string(), "venue-a".to_string()]);

    let said = outcome.expect_err("venue-z is not known").to_string();
    assert!(said.contains("venue-a"), "{said}");
    assert!(said.contains("venue-b"), "{said}");
    assert!(
        said.find("venue-a") < said.find("venue-b"),
        "listed in an order an operator can scan: {said}"
    );
}

#[test]
fn absent_is_refused_rather_than_defaulted() {
    let outcome: Result<u32, _> = required("limits.freshness_bound_ns", None);

    assert_eq!(
        outcome.expect_err("unset").reason,
        Reason::Absent,
        "where no safe resolution exists the platform must not start rather than pick one"
    );
}

#[test]
fn a_zero_is_not_positive() {
    assert!(positive("desk.maximum", 0).is_err());
    assert!(positive("desk.maximum", -1).is_err());
    assert_eq!(positive("desk.maximum", 1).expect("positive"), 1);
}

#[test]
fn a_repeated_name_is_refused() {
    let venues = vec!["venue-a".to_string(), "venue-a".to_string()];

    assert!(matches!(
        distinct("desk.venues", &venues).expect_err("repeated").reason,
        Reason::Duplicate { .. }
    ));
}

#[test]
fn an_empty_list_a_safety_property_depends_on_is_refused() {
    let venues: [String; 0] = [];

    assert!(non_empty("desk.venues", &venues).is_err());
    assert!(non_empty("desk.venues", &["venue-a".to_string()]).is_ok());
}

#[test]
fn a_value_outside_its_range_names_the_range() {
    let said = within("price_scale", 40, 0, 18).expect_err("out of range").to_string();

    assert!(said.contains("0..=18"), "{said}");
}
