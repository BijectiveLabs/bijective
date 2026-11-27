// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reconciliation on the order path: what evidence settles is recorded, survives a restart, and
//! decides whether an operator may resume.

use bijective_authority::Role;
use bijective_event_store::store::StreamWriter;
use bijective_lifecycle::{
    Fill, FillReport, Liquidity, OrderState, OrderStatusReport, PositionEvidence, PositionReport,
    ReportedStatus, VenueEvidence, Verdict,
};
use bijective_limits::SafeState;
use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_serialization::signal::{Command, Instruction};
use bijective_venue::normalise::{Unknown, VenueOutcome};

use crate::path::OrderPath;
use crate::wire::{ControlEvent, Settlement, decode, encode};

fn root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-reconcile-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `c-1` on `venue-a` and `c-2` on `venue-b`, both timed out, so both are unknown.
fn two_unknown(root: &std::path::Path) {
    let mut w = StreamWriter::open(root, "control").unwrap();
    for (n, (client, venue)) in [("c-1", "venue-a"), ("c-2", "venue-b")].into_iter().enumerate() {
        let intent = ControlEvent::Intent {
            decision_id: format!("d-{client}"),
            venue: Some(venue.into()),
            owner: None,
            request: OrderRequest {
                client_id: client.into(),
                instrument: "aaa".into(),
                side: Side::Buy,
                quantity_scaled: 10,
                price_scaled: Some(100),
                reduce_only: false,
                terms: Default::default(),
            },
            from_position: n as u64,
            at_ns: 1,
        };
        let settled = ControlEvent::Settled {
            client_id: client.into(),
            outcome: VenueOutcome::Unknown(Unknown::Timeout),
            at_ns: 2,
        };
        w.append(&encode(&intent).unwrap()).unwrap();
        w.append(&encode(&settled).unwrap()).unwrap();
    }
}

fn open(root: &std::path::Path) -> OrderPath {
    OrderPath::open(root, "control", "limits").expect("open")
}

fn filled_evidence(client: &str, position: i128) -> VenueEvidence {
    VenueEvidence {
        charges: Vec::new(),
        orders: vec![OrderStatusReport {
            client_id: Some(client.into()),
            venue_order_id: "v-1".into(),
            instrument: VenueSymbol::new("aaa"),
            side: Side::Buy,
            ordered_quantity_scaled: 10,
            filled_quantity_scaled: 10,
            status: ReportedStatus::Filled,
            reported_at: ExchangeTime::At { at_ns: 3 },
        }],
        fills: vec![FillReport {
            client_id: Some(client.into()),
            instrument: VenueSymbol::new("aaa"),
            fill: Fill {
                fill_id: "f-1".into(),
                venue_order_id: Some("v-1".into()),
                side: Side::Buy,
                price_scaled: 100,
                quantity_scaled: 10,
                fee: None,
                liquidity: Liquidity::Taker,
                exchange_time: ExchangeTime::At { at_ns: 3 },
                received_at_ns: 4,
            },
        }],
        positions: PositionEvidence::Reported(vec![PositionReport {
            instrument: VenueSymbol::new("aaa"),
            quantity_scaled: position,
        }]),
        history_from_ns: None,
        complete: true,
    }
}

fn resume(path: &mut OrderPath) -> SafeState {
    let command = Command {
        id: "cmd".into(),
        actor: "duty".into(),
        role: Role::Operator,
        instruction: Instruction::Resume,
        at_ns: 9,
    };
    path.carry_out(&command, None, 0, 9).unwrap()
}

#[test]
fn both_new_events_round_trip() {
    let events = [
        ControlEvent::Reconciled {
            venue: "venue-a".into(),
            client_id: "c-1".into(),
            settlement: Settlement::Accepted { venue_order_id: Some("v-1".into()) },
            at_ns: 5,
        },
        ControlEvent::Reconciled {
            venue: "venue-a".into(),
            client_id: "c-1".into(),
            settlement: Settlement::Rejected { reason: "refused".into() },
            at_ns: 5,
        },
        ControlEvent::ReconciliationCompleted {
            venue: "venue-a".into(),
            verdict: Verdict::Diverged,
            findings: vec!["one".into(), "two".into()],
            evidence: crate::wire::EvidenceRead::Failed,
            at_ns: 6,
        },
    ];
    for event in events {
        let bytes = encode(&event).unwrap();
        assert_eq!(decode(&bytes).unwrap(), event);
    }
}

#[test]
fn an_unknown_order_settled_by_evidence_stays_settled_after_a_restart() {
    let r = root("settled");
    two_unknown(&r);
    {
        let mut path = open(&r);
        let done = path.reconcile_venue("venue-a", &filled_evidence("c-1", 10), 5).unwrap();
        assert_eq!(done.verdict, Verdict::Aligned, "{:?}", done.findings);
        assert_eq!(done.recorded, 3, "reconciling, accepted, filled");
    }
    let path = open(&r);
    let ledger = path.ledger_for_tests("venue-a");
    let order = ledger.order("c-1").unwrap();
    assert_eq!((order.state, order.filled_quantity_scaled), (OrderState::Filled, 10));
    assert_eq!(ledger.position(&VenueSymbol::new("aaa")).unwrap().quantity_scaled(), 10);
    // The other venue's order is untouched: evidence from one venue settles only its own orders.
    assert_eq!(path.ledger_for_tests("venue-b").order("c-2").unwrap().state, OrderState::Unknown);
    assert_eq!(path.orders().0.unknown, 1);
}

#[test]
fn resume_waits_for_every_unknown_order_and_then_opens() {
    let r = root("resume");
    two_unknown(&r);
    let mut path = open(&r);
    path.reconcile_venue("venue-a", &filled_evidence("c-1", 10), 5).unwrap();
    assert_eq!(resume(&mut path), SafeState::Halted, "c-2 on venue-b is still unknown");
    path.reconcile_venue("venue-b", &filled_evidence("c-2", 10), 6).unwrap();
    assert_eq!(resume(&mut path), SafeState::Active);
}

#[test]
fn a_divergence_holds_resume_until_a_pass_agrees_and_an_unreadable_venue_does_not_clear_it() {
    let r = root("diverged");
    two_unknown(&r);
    let mut path = open(&r);
    // The venue holds 25 where the fills build 10: capital this platform did not place.
    let diverged = path.reconcile_venue("venue-a", &filled_evidence("c-1", 25), 5).unwrap();
    assert_eq!(diverged.verdict, Verdict::Diverged);
    path.reconcile_venue("venue-b", &filled_evidence("c-2", 10), 5).unwrap();
    assert_eq!(resume(&mut path), SafeState::Halted);

    let not_published = crate::wire::EvidenceRead::NotPublished;
    path.reconciliation_unavailable("venue-a", "the venue publishes nothing", not_published, 6)
        .unwrap();
    drop(path);
    let mut path = open(&r);
    assert_eq!(resume(&mut path), SafeState::Halted, "an unverified pass did not clear it");

    let mut agreeing = filled_evidence("c-1", 10);
    agreeing.orders.clear();
    agreeing.fills.clear();
    let aligned = path.reconcile_venue("venue-a", &agreeing, 7).unwrap();
    assert_eq!(aligned.verdict, Verdict::Aligned, "{:?}", aligned.findings);
    assert_eq!(resume(&mut path), SafeState::Active);
}

/// Every action an operator can issue must read back from the record of carrying it out. One that
/// did not would make the control stream unreadable at the next start.
#[test]
fn a_record_of_every_action_reads_back() {
    for action in bijective_authority::Action::ALL {
        let event = ControlEvent::CommandApplied {
            command_id: "cmd".into(),
            actor: "duty".into(),
            role: Role::Operator,
            action,
            state: SafeState::Halted,
            detail: String::new(),
            from_position: 0,
            at_ns: 1,
        };
        let bytes = encode(&event).unwrap();
        assert_eq!(decode(&bytes).unwrap(), event, "{}", action.as_str());
    }
}

/// A venue withdrawn by an operator leaves a record the next start can read.
#[test]
fn a_node_that_withdrew_a_venue_starts_again() {
    let r = root("withdrawn-restart");
    {
        let mut path = open(&r);
        let command = Command {
            id: "cmd-w".into(),
            actor: "duty".into(),
            role: Role::Operator,
            instruction: Instruction::WithdrawVenue { venue: "venue-a".into() },
            at_ns: 1,
        };
        path.carry_out(&command, None, 0, 1).unwrap();
    }
    let reopened = OrderPath::open(&r, "control", "limits");
    assert!(reopened.is_ok(), "{:?}", reopened.err());
}

/// A venue stays due for another pass while an order there is unknown or its last pass diverged,
/// and drops out once a pass settles it.
#[test]
fn a_venue_is_due_again_until_a_pass_settles_it() {
    let r = root("unsettled");
    two_unknown(&r);
    let mut path = open(&r);
    assert_eq!(path.unsettled_venues(), vec!["venue-a".to_string(), "venue-b".to_string()]);

    path.reconcile_venue("venue-a", &filled_evidence("c-1", 10), 5).unwrap();
    assert_eq!(path.unsettled_venues(), vec!["venue-b".to_string()]);

    path.reconcile_venue("venue-b", &filled_evidence("c-2", 30), 6).unwrap();
    assert_eq!(path.unsettled_venues(), vec!["venue-b".to_string()], "diverged keeps it due");
}

/// A read that failed has said nothing about the venue: resume waits for a read that succeeds,
/// and the venue stays due for another pass. One that publishes nothing does not hold resume.
#[test]
fn a_failed_read_holds_resume_until_a_read_succeeds() {
    let r = root("unread");
    drop(StreamWriter::open(&r, "control").unwrap());
    let mut path = open(&r);
    let failed = crate::wire::EvidenceRead::Failed;
    path.reconciliation_unavailable("venue-a", "connection refused", failed, 1).unwrap();
    assert_eq!(resume(&mut path), SafeState::Halted);
    assert_eq!(path.unsettled_venues(), vec!["venue-a".to_string()]);
    drop(path);

    let mut path = open(&r);
    assert_eq!(resume(&mut path), SafeState::Halted, "the failed read survives a restart");
    let flat = VenueEvidence {
        charges: Vec::new(),
        orders: vec![],
        fills: vec![],
        positions: PositionEvidence::Reported(vec![]),
        history_from_ns: None,
        complete: true,
    };
    path.reconcile_venue("venue-a", &flat, 2).unwrap();
    assert!(path.unsettled_venues().is_empty());
    assert_eq!(resume(&mut path), SafeState::Active);

    let quiet = root("quiet");
    drop(StreamWriter::open(&quiet, "control").unwrap());
    let mut path = open(&quiet);
    let none = crate::wire::EvidenceRead::NotPublished;
    path.reconciliation_unavailable("venue-b", "publishes no evidence", none, 1).unwrap();
    assert_eq!(resume(&mut path), SafeState::Active);
}

fn safe_states(root: &std::path::Path) -> Vec<bijective_limits::SafeStateReason> {
    let last = bijective_event_store::store::end_of(root, "control").unwrap().unwrap_or(0);
    bijective_event_store::store::read_range(root, "control", 0, last)
        .unwrap()
        .iter()
        .filter_map(|e| match decode(&e.payload).unwrap() {
            ControlEvent::SafeStateDriven { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// Found while trading, a divergence or an unread venue halts the platform with its reason, once.
#[test]
fn a_check_that_finds_a_divergence_while_trading_halts_once() {
    let r = root("halts");
    drop(StreamWriter::open(&r, "control").unwrap());
    let mut path = open(&r);
    path.resume(SafeState::Active);
    let mut held = filled_evidence("c-9", 25);
    held.orders.clear();
    held.fills.clear();
    path.reconcile_venue("venue-a", &held, 1).unwrap();
    assert_eq!(path.trading_state(), SafeState::Halted);
    path.reconcile_venue("venue-a", &held, 2).unwrap();
    assert_eq!(safe_states(&r), vec![bijective_limits::SafeStateReason::ReconciliationDiverged]);

    let unread = root("halts-unread");
    drop(StreamWriter::open(&unread, "control").unwrap());
    let mut path = open(&unread);
    path.resume(SafeState::Active);
    let failed = crate::wire::EvidenceRead::Failed;
    path.reconciliation_unavailable("venue-a", "timed out", failed, 1).unwrap();
    assert_eq!(path.trading_state(), SafeState::Halted);
    assert_eq!(safe_states(&unread), vec![bijective_limits::SafeStateReason::EvidenceUnread]);
}

/// Evidence reaches back to the latest aligned pass, not to the first intent forever. A pass that
/// did not agree does not move it, and the clock margin stays below it.
#[test]
fn evidence_reaches_back_to_the_latest_agreement_less_the_clock_margin() {
    let r = root("watermark");
    two_unknown(&r);
    let mut path = open(&r);
    let margin = crate::path::EVIDENCE_CLOCK_MARGIN_NS;
    assert_eq!(
        path.evidence_since_ns("venue-a"),
        Some(1 - margin),
        "the first intent, before any pass"
    );

    let aligned_at = 500_000_000_000;
    let done = path.reconcile_venue("venue-a", &filled_evidence("c-1", 10), aligned_at).unwrap();
    assert_eq!(done.verdict, Verdict::Aligned, "{:?}", done.findings);
    assert_eq!(path.evidence_since_ns("venue-a"), Some(aligned_at - margin));

    let diverged =
        path.reconcile_venue("venue-a", &filled_evidence("c-1", 25), aligned_at + 9).unwrap();
    assert_eq!(diverged.verdict, Verdict::Diverged);
    assert_eq!(
        path.evidence_since_ns("venue-a"),
        Some(aligned_at - margin),
        "only agreement moves it"
    );
    assert_eq!(path.evidence_since_ns("venue-b"), Some(1 - margin), "each venue keeps its own");
    drop(path);
    assert_eq!(
        open(&r).evidence_since_ns("venue-a"),
        Some(aligned_at - margin),
        "read back after a restart"
    );
}

/// A pass that saw an order filled ahead of its fills owes the venue a read on the next pass. The
/// pass that finds the fills clears it.
#[test]
fn a_venue_with_fills_pending_is_read_again_until_they_arrive() {
    let r = root("awaiting");
    two_unknown(&r);
    let mut path = open(&r);
    let mut early = filled_evidence("c-1", 10);
    early.fills.clear();
    let at = 1_000;
    let pending = path.reconcile_venue("venue-a", &early, at).unwrap();
    assert_eq!(pending.verdict, Verdict::Unverified, "{:?}", pending.findings);
    assert_eq!(path.awaiting_fills(), vec!["venue-a".to_string()]);

    path.reconcile_venue("venue-a", &filled_evidence("c-1", 10), at + 5).unwrap();
    assert!(path.awaiting_fills().is_empty());
}

fn charged(evidence: &mut VenueEvidence, id: &str) {
    evidence.charges.push(bijective_lifecycle::AccountCharge {
        charge_id: id.into(),
        fee: bijective_lifecycle::Fee { asset: "USD".into(), amount_scaled: 2, scale: 2 },
    });
}

/// A charge read again in a later window, or after a restart, is recorded once.
#[test]
fn an_account_charge_is_recorded_once_and_survives_a_restart() {
    let dir = root("charge");
    two_unknown(&dir);
    let mut evidence = filled_evidence("c-1", 10);
    charged(&mut evidence, "fee-1");
    let mut path = open(&dir);
    path.reconcile_venue("venue-a", &evidence, 10).expect("reconciled");
    path.reconcile_venue("venue-a", &evidence, 11).expect("reconciled");
    drop(path);

    let mut path = open(&dir);
    charged(&mut evidence, "fee-2");
    path.reconcile_venue("venue-a", &evidence, 12).expect("reconciled");

    let orders = path.recorded_orders().expect("rebuilt");
    let at: Vec<i128> = orders.account_charged_since(0).map(|c| c.at_ns).collect();
    assert_eq!(at, vec![10, 12]);
    assert!(orders.has_charge("venue-a", "fee-1"));
    assert!(!orders.has_charge("venue-b", "fee-1"));
}

#[test]
fn an_account_charge_round_trips() {
    let event = ControlEvent::AccountCharged {
        venue: "venue-a".into(),
        charge: bijective_lifecycle::AccountCharge {
            charge_id: "fee-1".into(),
            fee: bijective_lifecycle::Fee { asset: "USD".into(), amount_scaled: -7, scale: 8 },
        },
        at_ns: 5,
    };
    assert_eq!(decode(&encode(&event).unwrap()).unwrap(), event);
}
