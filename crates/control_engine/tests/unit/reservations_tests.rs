// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A finished order returns what it reserved beyond its filled share: once, and after a crash.

use bijective_event_store::store::StreamWriter;
use bijective_lifecycle::{
    Fill, FillReport, Liquidity, OrderStatusReport, PositionEvidence, PositionReport,
    ReportedStatus, VenueEvidence,
};
use bijective_limits::{Amount, LimitKind, Scope, Threshold};
use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_risk::limit_state::{Budget, LimitEvent, encode as encode_limit};
use bijective_venue::normalise::{Unknown, VenueOutcome};

use crate::path::OrderPath;
use crate::wire::{ControlEvent, encode};

fn root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-reserve-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn scope() -> Scope {
    Scope::Venue("venue-a".into())
}

/// 1,000 of gross exposure declared, 100 reserved by `d-1` for an order of 10 that timed out.
fn reserved_and_unknown(root: &std::path::Path) {
    let mut limits = StreamWriter::open(root, "limits").unwrap();
    let declared = LimitEvent::Declared {
        scope: scope(),
        kind: LimitKind::GrossExposure,
        maximum: Threshold::Money(Amount::from_scaled(1_000)),
        config_version: 1,
    };
    let consumed = LimitEvent::Consumed {
        scope: scope(),
        kind: LimitKind::GrossExposure,
        amount: Amount::from_scaled(100),
        decision_id: "d-1".into(),
    };
    limits.append(&encode_limit(&declared).unwrap()).unwrap();
    limits.append(&encode_limit(&consumed).unwrap()).unwrap();

    let mut control = StreamWriter::open(root, "control").unwrap();
    let intent = ControlEvent::Intent {
        decision_id: "d-1".into(),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: "c-1".into(),
            instrument: "aaa".into(),
            side: Side::Buy,
            quantity_scaled: 10,
            price_scaled: Some(10),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position: 0,
        at_ns: 1,
    };
    let settled = ControlEvent::Settled {
        client_id: "c-1".into(),
        outcome: VenueOutcome::Unknown(Unknown::Timeout),
        at_ns: 2,
    };
    control.append(&encode(&intent).unwrap()).unwrap();
    control.append(&encode(&settled).unwrap()).unwrap();
}

/// The venue reports the order ended `status` with `filled` of 10 done.
fn ended(status: ReportedStatus, filled: i128) -> VenueEvidence {
    let fills = (filled > 0)
        .then(|| FillReport {
            client_id: Some("c-1".into()),
            instrument: VenueSymbol::new("aaa"),
            fill: Fill {
                fill_id: "f-1".into(),
                venue_order_id: Some("v-1".into()),
                side: Side::Buy,
                price_scaled: 10,
                quantity_scaled: filled,
                fee: None,
                liquidity: Liquidity::Taker,
                exchange_time: ExchangeTime::At { at_ns: 3 },
                received_at_ns: 4,
            },
        })
        .into_iter()
        .collect();
    VenueEvidence {
        charges: Vec::new(),
        orders: vec![OrderStatusReport {
            client_id: Some("c-1".into()),
            venue_order_id: "v-1".into(),
            instrument: VenueSymbol::new("aaa"),
            side: Side::Buy,
            ordered_quantity_scaled: 10,
            filled_quantity_scaled: filled,
            status,
            reported_at: ExchangeTime::At { at_ns: 3 },
        }],
        fills,
        positions: PositionEvidence::Reported(vec![PositionReport {
            instrument: VenueSymbol::new("aaa"),
            quantity_scaled: filled,
        }]),
        history_from_ns: None,
        complete: true,
    }
}

fn open(root: &std::path::Path) -> OrderPath {
    OrderPath::open(root, "control", "limits").expect("open")
}

fn remaining(path: &mut OrderPath) -> i128 {
    match path.limits_mut().0.budget(&scope(), LimitKind::GrossExposure) {
        Budget::Remaining(left) => left.scaled(),
        other => panic!("budget is {other:?}"),
    }
}

#[test]
fn a_cancelled_partial_fill_returns_the_unfilled_share_once() {
    let r = root("partial");
    reserved_and_unknown(&r);
    let mut path = open(&r);
    assert_eq!(remaining(&mut path), 900);
    path.reconcile_venue("venue-a", &ended(ReportedStatus::Canceled, 4), 5).unwrap();
    assert_eq!(remaining(&mut path), 960, "40 stays reserved for the 4 of 10 that filled");
    assert_eq!(path.settle_reservations().unwrap(), 0, "a second sweep releases nothing");
    drop(path);
    let mut path = open(&r);
    assert_eq!(remaining(&mut path), 960, "a restart releases nothing again");
}

#[test]
fn a_filled_order_keeps_its_whole_reservation() {
    let r = root("filled");
    reserved_and_unknown(&r);
    let mut path = open(&r);
    path.reconcile_venue("venue-a", &ended(ReportedStatus::Filled, 10), 5).unwrap();
    assert_eq!(remaining(&mut path), 900);
}

#[test]
fn an_order_still_unknown_keeps_its_whole_reservation() {
    let r = root("unknown");
    reserved_and_unknown(&r);
    let mut path = open(&r);
    assert_eq!(path.settle_reservations().unwrap(), 0);
    assert_eq!(remaining(&mut path), 900);
}

/// The order finished on the record and the process died before the release: the next start
/// returns it.
#[test]
fn a_release_lost_to_a_crash_is_made_at_the_next_start() {
    let r = root("crash");
    reserved_and_unknown(&r);
    {
        let mut control = StreamWriter::open(&r, "control").unwrap();
        let settle = |settlement| ControlEvent::Reconciled {
            venue: "venue-a".into(),
            client_id: "c-1".into(),
            settlement,
            at_ns: 5,
        };
        use crate::wire::Settlement;
        for event in [
            settle(Settlement::ReconciliationStarted),
            settle(Settlement::Accepted { venue_order_id: Some("v-1".into()) }),
            settle(Settlement::Canceled),
        ] {
            control.append(&encode(&event).unwrap()).unwrap();
        }
    }
    let mut path = open(&r);
    assert_eq!(remaining(&mut path), 1_000, "nothing filled, so the whole 100 came back");
}

/// A filled order keeps its share while the position is open and returns it once the position is
/// closed, recorded as closed rather than cancelled.
#[test]
fn a_closed_position_returns_what_its_fills_kept() {
    let r = root("closed");
    reserved_and_unknown(&r);
    let mut path = open(&r);
    path.reconcile_venue("venue-a", &ended(ReportedStatus::Filled, 10), 5).unwrap();
    assert_eq!(remaining(&mut path), 900);

    let close = bijective_serialization::signal::Command {
        id: "adj-close".into(),
        actor: "risk-desk".into(),
        role: bijective_authority::Role::RiskOfficer,
        instruction: bijective_serialization::signal::Instruction::AdjustPosition {
            venue: "venue-a".into(),
            adjustment: bijective_serialization::signal::PositionAdjustment {
                instrument: "aaa".into(),
                side: Side::Sell,
                quantity_scaled: 10,
                price_scaled: 10,
                reason: "closed by transfer for the test".into(),
                owner: None,
            },
        },
        at_ns: 6,
    };
    path.carry_out(&close, None, 0, 6).unwrap();
    assert_eq!(remaining(&mut path), 1_000, "returned when the position closed, not at a restart");
    drop(path);
    let mut path = open(&r);
    assert_eq!(remaining(&mut path), 1_000);

    let last = bijective_event_store::store::end_of(&r, "limits").unwrap().unwrap();
    let closed = bijective_event_store::store::read_range(&r, "limits", 0, last)
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                bijective_risk::limit_state::decode(&e.payload),
                Ok(LimitEvent::Released {
                    reason: bijective_risk::limit_state::ReleaseReason::PositionClosed,
                    ..
                })
            )
        })
        .count();
    assert_eq!(closed, 1);
}
