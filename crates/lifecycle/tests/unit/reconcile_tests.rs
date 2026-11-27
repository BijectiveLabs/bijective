// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reconciliation: each rule shown by the evidence that triggers it.

use bijective_model::market::ExchangeTime;
use bijective_model::{Side, VenueSymbol};

use crate::ledger::Ledger;
use crate::order::{OrderEvent, OrderState};
use crate::order_tests::{fill, request};
use crate::reconcile::{FILL_LAG_NS, Finding, Verdict, reconcile};
use crate::report::{
    FillReport, OrderStatusReport, PositionEvidence, PositionReport, ReportedStatus, VenueEvidence,
};

fn unknown() -> Ledger {
    let mut l = Ledger::default();
    l.create(request(10)).unwrap();
    l.apply("bl-1", &OrderEvent::Submitted, 1).unwrap();
    l.apply("bl-1", &OrderEvent::OutcomeUnknown { detail: "timeout".into() }, 2).unwrap();
    l
}

fn status(filled: i128, status: ReportedStatus) -> OrderStatusReport {
    OrderStatusReport {
        client_id: Some("bl-1".into()),
        venue_order_id: "v-1".into(),
        instrument: VenueSymbol::new("AAA"),
        side: Side::Buy,
        ordered_quantity_scaled: 10,
        filled_quantity_scaled: filled,
        status,
        reported_at: ExchangeTime::At { at_ns: 9 },
    }
}

fn filled(id: &str, quantity: i128) -> FillReport {
    FillReport {
        client_id: Some("bl-1".into()),
        instrument: VenueSymbol::new("AAA"),
        fill: fill(id, quantity),
    }
}

fn evidence(
    orders: Vec<OrderStatusReport>,
    fills: Vec<FillReport>,
    position: Option<i128>,
) -> VenueEvidence {
    VenueEvidence {
        charges: Vec::new(),
        orders,
        fills,
        positions: PositionEvidence::Reported(
            position
                .map(|q| PositionReport { instrument: VenueSymbol::new("AAA"), quantity_scaled: q })
                .into_iter()
                .collect(),
        ),
        history_from_ns: None,
        complete: true,
    }
}

/// An unknown order the venue reports filled, with the fills that did it, reconciles through
/// `Reconciling` to `Filled`, and the position built from those fills matches the venue's.
#[test]
fn an_unknown_order_is_settled_by_evidence_and_the_position_agrees() {
    let r = reconcile(
        &unknown(),
        &evidence(vec![status(10, ReportedStatus::Filled)], vec![filled("f-1", 10)], Some(10)),
        5,
    );
    assert_eq!(r.verdict, Verdict::Aligned, "{:?}", r.findings);
    assert_eq!(r.ledger.order("bl-1").unwrap().state, OrderState::Filled);
    let path: Vec<(OrderState, OrderState)> = r.applied.iter().map(|t| (t.from, t.to)).collect();
    assert_eq!(
        path,
        vec![
            (OrderState::Unknown, OrderState::Reconciling),
            (OrderState::Reconciling, OrderState::Accepted),
            (OrderState::Accepted, OrderState::Filled),
        ]
    );
}

/// Absent from the report, an unknown order stays unknown: absence is not failure.
#[test]
fn an_unknown_order_the_venue_did_not_mention_stays_unresolved() {
    let r = reconcile(&unknown(), &evidence(vec![], vec![], Some(0)), 5);
    assert_eq!(r.verdict, Verdict::Diverged);
    assert_eq!(r.findings, vec![Finding::Unresolved { client_id: "bl-1".into() }]);
    assert_eq!(r.ledger.order("bl-1").unwrap().state, OrderState::Unknown);
}

/// Quantity the venue says filled without the fills that explain it is a finding, never an
/// invented fill.
#[test]
fn filled_quantity_without_fills_is_a_finding_not_a_fill() {
    let r = reconcile(
        &unknown(),
        &evidence(vec![status(4, ReportedStatus::Working)], vec![], Some(4)),
        9 + FILL_LAG_NS,
    );
    assert!(r.findings.contains(&Finding::MissingFills {
        client_id: "bl-1".into(),
        reported_scaled: 4,
        held_scaled: 0
    }));
    assert_eq!(r.ledger.order("bl-1").unwrap().filled_quantity_scaled, 0);
    assert_eq!(r.verdict, Verdict::Diverged);
}

/// A venue lists executions after the status they explain: a young gap holds the verdict at
/// unverified, with the position on that instrument waiting too, and invents no fill.
#[test]
fn a_young_gap_between_status_and_fills_is_unverified_not_diverged() {
    let r = reconcile(
        &unknown(),
        &evidence(vec![status(4, ReportedStatus::Working)], vec![], Some(4)),
        9 + FILL_LAG_NS - 1,
    );
    assert!(r.findings.contains(&Finding::FillsPending {
        client_id: "bl-1".into(),
        reported_scaled: 4,
        held_scaled: 0
    }));
    assert!(r.findings.contains(&Finding::PositionAwaitingFills {
        instrument: "AAA".into(),
        venue_scaled: 4,
        ledger_scaled: 0
    }));
    assert_eq!(r.ledger.order("bl-1").unwrap().filled_quantity_scaled, 0);
    assert_eq!(r.verdict, Verdict::Unverified);
}

/// A report with no venue stamp has no age to be young by.
#[test]
fn an_unstamped_gap_is_a_divergence() {
    let mut unstamped = status(4, ReportedStatus::Working);
    unstamped.reported_at = ExchangeTime::NotPublished;
    let r = reconcile(&unknown(), &evidence(vec![unstamped], vec![], Some(4)), 9);
    assert!(r.findings.iter().any(|f| matches!(f, Finding::MissingFills { .. })));
    assert_eq!(r.verdict, Verdict::Diverged);
}

/// A snapshot taken before a fill the stream delivered does not undo the fill.
#[test]
fn an_older_snapshot_does_not_undo_a_fill() {
    let mut l = unknown();
    l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 5)), 3).unwrap();
    let r = reconcile(&l, &evidence(vec![status(0, ReportedStatus::Working)], vec![], Some(5)), 5);
    assert_eq!(r.ledger.order("bl-1").unwrap().filled_quantity_scaled, 5);
    assert!(r.findings.iter().any(|f| matches!(f, Finding::StaleReport { .. })));
    assert_eq!(r.verdict, Verdict::Aligned, "a stale snapshot is information, not divergence");
}

#[test]
fn an_order_or_fill_this_platform_did_not_place_is_external() {
    let mut foreign = status(0, ReportedStatus::Working);
    foreign.client_id = None;
    let mut stray = filled("f-9", 1);
    stray.client_id = Some("not-ours".into());
    let r = reconcile(&Ledger::default(), &evidence(vec![foreign], vec![stray], Some(0)), 5);
    assert_eq!(r.findings.iter().filter(|f| matches!(f, Finding::External { .. })).count(), 2);
    assert_eq!(r.verdict, Verdict::Diverged);
}

/// An explicit position is authoritative. A missing one is not flat. None published is unverified.
#[test]
fn positions_are_judged_only_on_explicit_reports() {
    let mut l = unknown();
    l.apply("bl-1", &OrderEvent::Filled(fill("f-1", 10)), 3).unwrap();
    let settled = vec![status(10, ReportedStatus::Filled)];

    let differs = reconcile(&l, &evidence(settled.clone(), vec![], Some(7)), 5);
    assert!(differs.findings.contains(&Finding::PositionMismatch {
        instrument: "AAA".into(),
        venue_scaled: 7,
        ledger_scaled: 10
    }));

    let missing = reconcile(&l, &evidence(settled.clone(), vec![], None), 5);
    assert!(
        missing.findings.contains(&Finding::PositionUnconfirmed {
            instrument: "AAA".into(),
            ledger_scaled: 10
        })
    );

    let mut silent = evidence(settled, vec![], None);
    silent.positions = PositionEvidence::NotPublished;
    assert_eq!(reconcile(&l, &silent, 5).verdict, Verdict::Unverified);
}

#[test]
fn incomplete_evidence_cannot_align() {
    let mut e =
        evidence(vec![status(10, ReportedStatus::Filled)], vec![filled("f-1", 10)], Some(10));
    e.complete = false;
    assert_eq!(reconcile(&unknown(), &e, 5).verdict, Verdict::Unverified);
}

/// A canceled order with a late partial fill: the fill counts before the cancel closes it.
#[test]
fn a_cancel_report_closes_the_order_after_its_late_fills() {
    let r = reconcile(
        &unknown(),
        &evidence(vec![status(3, ReportedStatus::Canceled)], vec![filled("f-1", 3)], Some(3)),
        5,
    );
    let order = r.ledger.order("bl-1").unwrap();
    assert_eq!((order.state, order.filled_quantity_scaled), (OrderState::Canceled, 3));
    assert_eq!(r.verdict, Verdict::Aligned, "{:?}", r.findings);
}

/// Reconciliation is pure: the ledger it was given is unchanged.
#[test]
fn the_ledger_passed_in_is_not_changed() {
    let l = unknown();
    let before = l.clone();
    reconcile(
        &l,
        &evidence(vec![status(10, ReportedStatus::Filled)], vec![filled("f-1", 10)], Some(10)),
        5,
    );
    assert_eq!(l, before);
}

#[test]
fn an_order_the_venue_holds_at_another_quantity_is_a_finding() {
    let mut report = status(0, ReportedStatus::Working);
    report.ordered_quantity_scaled = 12;
    let r = reconcile(&unknown(), &evidence(vec![report], vec![], Some(0)), 5);
    let said: Vec<String> = r.findings.iter().map(ToString::to_string).collect();
    assert!(said.iter().any(|f| f.contains("order of 12, the platform placed 10")), "{said:?}");
    assert_eq!(r.verdict, Verdict::Diverged);
}

/// The events a pass reports, applied to the ledger it started from, rebuild the ledger it
/// returned: what a caller records is exactly what the pass decided.
#[test]
fn the_reported_events_replayed_on_the_starting_ledger_reproduce_the_result() {
    let start = unknown();
    let r = reconcile(
        &start,
        &evidence(vec![status(10, ReportedStatus::Filled)], vec![filled("f-1", 10)], Some(10)),
        5,
    );
    assert_eq!(r.events.len(), r.applied.len());
    let mut replayed = start.clone();
    for (client_id, event) in &r.events {
        replayed.apply(client_id, event, 5).unwrap();
    }
    assert_eq!(replayed, r.ledger);
    assert_ne!(replayed, start, "the pass changed something, so the comparison is not vacuous");
}

/// The same fills listed in another order build the same ledger: they are applied in the order
/// they executed, not the order the adapter happened to page them.
#[test]
fn fills_apply_in_the_order_they_executed() {
    let at = |id: &str, quantity: i128, price: i128, at_ns: i128| {
        let mut report = filled(id, quantity);
        report.fill.price_scaled = price;
        report.fill.exchange_time = ExchangeTime::At { at_ns };
        report
    };
    let early = at("f-2", 4, 10_000, 5);
    let late = at("f-1", 6, 12_000, 9);
    let listed = |fills| evidence(vec![status(10, ReportedStatus::Filled)], fills, Some(10));
    let in_order = reconcile(&unknown(), &listed(vec![early.clone(), late.clone()]), 20);
    let reversed = reconcile(&unknown(), &listed(vec![late, early]), 20);
    assert_eq!(in_order.verdict, Verdict::Aligned, "{:?}", in_order.findings);
    assert_eq!(reversed.ledger, in_order.ledger);
    assert_eq!(reversed.events, in_order.events);
}
