// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One test per acceptance criterion, and each divergence class constructed deliberately.

use super::*;

const NOW: i128 = 1_700_000_000_000_000_000;
const WINDOW: i128 = 30_000_000_000;
const AS_OF: u64 = 4_211;

fn ours(instrument: &str, qty: i128, price: i128, opened_at_ns: i128) -> PlatformPosition {
    PlatformPosition {
        instrument: bijective_model::VenueSymbol::new(instrument),
        client_order_id: format!("bl-{instrument}-1"),
        venue_order_id: Some(format!("v-{instrument}-1")),
        quantity_scaled: qty,
        average_price_scaled: price,
        opened_at_ns,
    }
}

fn theirs(instrument: &str, qty: i128, price: i128) -> VenuePosition {
    VenuePosition {
        instrument: bijective_model::VenueSymbol::new(instrument),
        client_order_id: Some(format!("bl-{instrument}-1")),
        venue_order_id: Some(format!("v-{instrument}-1")),
        quantity_scaled: qty,
        average_price_scaled: price,
        updated_at_ms: (NOW / 1_000_000) as u64,
    }
}

fn run(o: &[PlatformPosition], t: &[VenuePosition], f: Fidelity, now: i128) -> Report {
    reconcile(o, t, f, WINDOW, AS_OF, now)
}

// Criterion 1: as of a stated position

#[test]
fn a_run_reports_the_position_it_matched_against() {
    let report = run(
        &[ours("BTCUSDT", 100, 76_000, NOW)],
        &[theirs("BTCUSDT", 100, 76_000)],
        Fidelity::VenueIdentifier,
        NOW,
    );

    assert_eq!(report.as_of, AS_OF);
    assert_eq!(report.matched, 1);
    assert!(report.is_clear());
}

#[test]
fn every_divergence_carries_the_position_it_was_judged_against() {
    let report = run(&[ours("BTCUSDT", 100, 76_000, 0)], &[], Fidelity::VenueIdentifier, NOW);

    assert_eq!(report.divergences.len(), 1);
    assert!(report.divergences.iter().all(|d| d.as_of == AS_OF));
}

// Criterion 2: a fallback match says so

#[test]
fn a_fallback_match_is_distinguishable_from_an_exact_one_in_the_record() {
    let mut venue = theirs("BTCUSDT", 200, 76_000);
    venue.venue_order_id = None;

    let exact = run(
        &[ours("BTCUSDT", 100, 76_000, NOW)],
        &[theirs("BTCUSDT", 200, 76_000)],
        Fidelity::VenueIdentifier,
        NOW,
    );
    let fallback = run(
        &[ours("BTCUSDT", 100, 76_000, NOW)],
        &[venue],
        Fidelity::Fallback(FallbackKey::ClientOrderId),
        NOW,
    );

    assert_eq!(exact.divergences[0].match_kind, Some(MatchKind::Exact));
    assert_eq!(
        fallback.divergences[0].match_kind,
        Some(MatchKind::Fallback(FallbackKey::ClientOrderId))
    );
}

// Criterion 3: all classes, each constructed deliberately

#[test]
fn a_record_we_hold_and_the_venue_does_not_is_missing_and_ours_is_kept() {
    let mine = ours("BTCUSDT", 100, 76_000, 0);
    let report = run(std::slice::from_ref(&mine), &[], Fidelity::VenueIdentifier, NOW);

    assert_eq!(report.divergences[0].class, DivergenceClass::Missing);
    assert_eq!(mine.quantity_scaled, 100, "our record is untouched by reconciling");
}

#[test]
fn a_record_the_venue_holds_and_we_do_not_is_extra() {
    let mut venue = theirs("ETHUSDT", 50, 2_650);
    venue.updated_at_ms = ((NOW - WINDOW * 2) / 1_000_000) as u64;

    let report = run(&[], &[venue], Fidelity::VenueIdentifier, NOW);
    assert_eq!(report.divergences[0].class, DivergenceClass::Extra);
}

#[test]
fn a_differing_field_is_raised_with_both_values_and_neither_adopted() {
    let mine = ours("BTCUSDT", 100, 76_000, NOW);
    let report = run(
        std::slice::from_ref(&mine),
        &[theirs("BTCUSDT", 140, 76_000)],
        Fidelity::VenueIdentifier,
        NOW,
    );

    assert_eq!(
        report.divergences[0].class,
        DivergenceClass::Value { field: "quantity", ours: 100, theirs: 140 }
    );
    assert_eq!(mine.quantity_scaled, 100, "the venue's value was not written into ours");
}

#[test]
fn a_record_inside_the_timing_window_is_in_flight_rather_than_missing() {
    let report = run(&[ours("BTCUSDT", 100, 76_000, NOW - 1)], &[], Fidelity::VenueIdentifier, NOW);
    assert_eq!(report.divergences[0].class, DivergenceClass::InFlight);
}

// Criterion 4: unmatchable is a profile defect, never a divergence

#[test]
fn an_unpairable_venue_record_routes_to_the_profile_and_never_to_divergences() {
    let mut venue = theirs("SOLUSDT", 10, 180);
    venue.venue_order_id = None;

    let report = run(&[], &[venue], Fidelity::VenueIdentifier, NOW);

    assert!(report.divergences.is_empty(), "a profile defect must not appear as a divergence");
    assert_eq!(report.profile_defects.len(), 1);
    assert!(report.profile_defects[0].detail.contains("declares venue identifiers"));
    assert!(!report.is_clear(), "a profile defect is still a finding");
}

#[test]
fn a_venue_with_no_declared_pairing_yields_defects_and_no_divergences() {
    let report = run(&[], &[theirs("BTCUSDT", 1, 1)], Fidelity::Undeclared, NOW);

    assert!(report.divergences.is_empty());
    assert_eq!(report.profile_defects.len(), 1);
}

// Criterion 5: in-flight is recomputed, and outliving the window alarms

#[test]
fn an_in_flight_divergence_that_outlives_its_window_is_reclassified() {
    let mine = [ours("BTCUSDT", 100, 76_000, NOW)];

    let early = run(&mine, &[], Fidelity::VenueIdentifier, NOW + WINDOW - 1);
    let late = run(&mine, &[], Fidelity::VenueIdentifier, NOW + WINDOW + 1);

    assert_eq!(early.divergences[0].class, DivergenceClass::InFlight);
    assert_eq!(late.divergences[0].class, DivergenceClass::Missing, "same input, later clock");
}

// Criterion 6: no path writes the venue's value into our record

/// A field-by-field copy would compile, so this reads the sources for it.
#[test]
fn no_module_that_sees_the_venue_ever_builds_one_of_our_records() {
    for (name, source) in [
        ("classify.rs", include_str!("../../src/classify.rs")),
        ("resolution.rs", include_str!("../../src/resolution.rs")),
        ("signal.rs", include_str!("../../src/signal.rs")),
    ] {
        assert!(
            !source.contains("PlatformPosition {"),
            "{name} constructs one of our records, which reconciliation forbids"
        );
        assert!(
            !source.contains("impl From<VenuePosition>"),
            "{name} declares a conversion from the venue's record into ours"
        );
    }
}

// Criterion 7: a resolution names a person

#[test]
fn a_resolution_without_a_named_person_cannot_be_recorded() {
    let divergence =
        run(&[ours("BTCUSDT", 1, 1, 0)], &[], Fidelity::VenueIdentifier, NOW).divergences.remove(0);

    assert_eq!(
        Resolution::record(divergence.clone(), "   ", "looked into it", NOW).unwrap_err(),
        ResolutionError::NoPersonNamed
    );
    assert_eq!(
        Resolution::record(divergence.clone(), "an operator", "", NOW).unwrap_err(),
        ResolutionError::NoReasonGiven
    );

    let sound = Resolution::record(divergence, "an operator", "venue settled late", NOW).unwrap();
    assert_eq!(sound.resolved_by(), "an operator");
}

// Criterion 8: the signal is driven by age, not by count

#[test]
fn open_divergences_carry_age_and_the_oldest_leads() {
    let one = run(&[ours("BTCUSDT", 1, 1, 0)], &[], Fidelity::VenueIdentifier, NOW).divergences;
    let mut old = one.clone();
    old[0].raised_at_ns = NOW - 90_000_000_000_000;

    let mut both = one;
    both.extend(old);
    let signal = open_signal(&both, NOW);

    assert_eq!(signal[0].severity, Severity::Overdue, "the oldest leads");
    assert!(signal[0].age_ns > signal[1].age_ns);
    assert_eq!(signal[1].severity, Severity::Raised);
}

#[test]
fn the_same_count_at_two_ages_produces_two_different_signals() {
    let fresh = run(&[ours("BTCUSDT", 1, 1, 0)], &[], Fidelity::VenueIdentifier, NOW).divergences;
    let stale = open_signal(&fresh, NOW + 90_000_000_000_000);
    let new = open_signal(&fresh, NOW);

    assert_eq!(stale.len(), new.len(), "the count is identical");
    assert_ne!(stale[0].severity, new[0].severity, "and the signal is not");
}

#[test]
fn an_extra_is_severe_from_the_moment_it_is_raised() {
    let mut venue = theirs("ETHUSDT", 50, 2_650);
    venue.updated_at_ms = ((NOW - WINDOW * 2) / 1_000_000) as u64;
    let divergences = run(&[], &[venue], Fidelity::VenueIdentifier, NOW).divergences;

    assert_eq!(open_signal(&divergences, NOW)[0].severity, Severity::Overdue);
}

// The venue's own account, read into the comparison

use bijective_model::PositionSnapshot;

fn snapshot(instrument: &str, qty: i128, entry: i128) -> PositionSnapshot {
    PositionSnapshot {
        instrument: bijective_model::VenueSymbol::new(instrument),
        quantity_scaled: qty,
        entry_price_scaled: entry,
    }
}

#[test]
fn an_account_position_becomes_the_venues_side_of_the_comparison() {
    let venue = VenuePosition::from_account(&snapshot("BTCUSDT", -250, 76_000), 1_700_000_000_000);

    assert_eq!(venue.instrument.as_str(), "BTCUSDT");
    assert_eq!(venue.quantity_scaled, -250, "short stays signed");
    assert_eq!(venue.average_price_scaled, 76_000);
}

/// Reconciliation, last row.
#[test]
fn a_position_carries_no_identifiers_and_says_so_by_routing_to_the_profile() {
    let venue = VenuePosition::from_account(&snapshot("BTCUSDT", 100, 76_000), 0);
    assert_eq!(venue.venue_order_id, None);
    assert_eq!(venue.client_order_id, None);

    let report = run(&[], &[venue], Fidelity::VenueIdentifier, NOW);
    assert!(report.divergences.is_empty(), "not a position divergence");
    assert_eq!(report.profile_defects.len(), 1, "a profile defect, routed to venue assurance");
}

#[test]
fn the_instrument_fallback_reconciles_a_real_account_against_ours() {
    let mine = ours("BTCUSDT", 100, 76_000, NOW);
    let theirs =
        VenuePosition::from_account(&snapshot("BTCUSDT", 100, 76_000), (NOW / 1_000_000) as u64);

    let clear = run(
        std::slice::from_ref(&mine),
        std::slice::from_ref(&theirs),
        Fidelity::Fallback(FallbackKey::Instrument),
        NOW,
    );
    assert!(clear.is_clear(), "matching books reconcile clean");
    assert_eq!(clear.matched, 1);

    let drifted =
        VenuePosition::from_account(&snapshot("BTCUSDT", 140, 76_000), (NOW / 1_000_000) as u64);
    let report = run(
        std::slice::from_ref(&mine),
        &[drifted],
        Fidelity::Fallback(FallbackKey::Instrument),
        NOW,
    );
    assert_eq!(
        report.divergences[0].class,
        DivergenceClass::Value { field: "quantity", ours: 100, theirs: 140 }
    );
    assert_eq!(
        report.divergences[0].match_kind,
        Some(MatchKind::Fallback(FallbackKey::Instrument)),
        "a fallback match says so"
    );
}

#[test]
fn a_position_we_do_not_hold_is_the_severe_class() {
    let theirs = VenuePosition::from_account(
        &snapshot("ETHUSDT", 50, 2_650),
        ((NOW - WINDOW * 2) / 1_000_000) as u64,
    );
    let report = run(&[], &[theirs], Fidelity::Fallback(FallbackKey::Instrument), NOW);

    assert_eq!(report.divergences[0].class, DivergenceClass::Extra);
    assert_eq!(open_signal(&report.divergences, NOW)[0].severity, Severity::Overdue);
}
