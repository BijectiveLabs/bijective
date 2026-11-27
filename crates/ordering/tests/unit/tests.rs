// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the audit must find, and what it must not invent.

use super::*;

use bijective_control_engine::wire::encode;
use bijective_event_store::store::StreamWriter;

const CONTROL: &str = "control";
const PROPOSALS: &str = "proposals";

fn store(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bijective-ordering-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn request(client_id: &str) -> bijective_model::OrderRequest {
    bijective_model::OrderRequest {
        client_id: client_id.to_string(),
        instrument: bijective_model::VenueSymbol::new("BTCUSDT"),
        side: bijective_model::Side::Buy,
        quantity_scaled: 1,
        price_scaled: Some(2_654_500),
        reduce_only: false,
        terms: Default::default(),
    }
}

fn intent(client_id: &str, from_position: u64, at_ns: i128) -> ControlEvent {
    ControlEvent::Intent {
        decision_id: format!("d-{client_id}"),
        venue: Some("venue-a".into()),
        owner: None,
        request: request(client_id),
        from_position,
        at_ns,
    }
}

fn settled(client_id: &str, at_ns: i128) -> ControlEvent {
    ControlEvent::Settled {
        client_id: client_id.to_string(),
        outcome: bijective_venue::normalise::VenueOutcome::Accepted,
        at_ns,
    }
}

/// Put proposals on the stream so the audit has something to check names against.
fn proposals(dir: &std::path::Path, how_many: usize) {
    let mut w = StreamWriter::open(dir, PROPOSALS).expect("proposals");
    for i in 0..how_many {
        let proposal = bijective_model::Proposal {
            desk: "alpha".into(),
            instrument: bijective_model::InstrumentId::new("btc-perp"),
            side: bijective_model::Side::Buy,
            quantity_scaled: 1,
            price_scaled: Some(2_654_500 + i as i128),
            cap_scaled: None,
            client_id: format!("c{i}"),
            decided_at_position: None,
            decided_at_ns: 1,
            venue: None,
            strategy: None,
            terms: Default::default(),
            reduce_only: false,
            parent: None,
            oco: None,
        };
        w.append(&bijective_serialization::signal::encode(&proposal).expect("encode"))
            .expect("append");
    }
}

fn recorded(dir: &std::path::Path, events: &[ControlEvent]) {
    let mut w = StreamWriter::open(dir, CONTROL).expect("control");
    for event in events {
        w.append(&encode(event).expect("encode")).expect("append");
    }
}

#[test]
fn a_record_in_order_reports_nothing_and_says_what_it_read() {
    let dir = store("clean");
    proposals(&dir, 2);
    recorded(
        &dir,
        &[intent("c0", 0, 10), settled("c0", 20), intent("c1", 1, 30), settled("c1", 40)],
    );

    let (read, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    assert_eq!(read, 4, "a pass over nothing must not read like a pass over a record");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_settlement_before_its_intent_is_found_with_the_distance() {
    let dir = store("before");
    proposals(&dir, 1);
    recorded(&dir, &[intent("c0", 0, 100), settled("c0", 40)]);

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    match findings.first() {
        Some(Finding::SettledBeforeItsIntent { intent_at_ns, settled_at_ns, .. }) => {
            assert_eq!((*intent_at_ns, *settled_at_ns), (100, 40));
        }
        other => panic!("a settlement before its intent must be found: {other:?}"),
    }
}

#[test]
fn a_settlement_with_no_intent_is_found() {
    let dir = store("orphan");
    proposals(&dir, 1);
    recorded(&dir, &[settled("c9", 40)]);

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    assert!(matches!(findings.first(), Some(Finding::SettledWithNoIntent { .. })), "{findings:?}");
}

#[test]
fn one_proposal_acted_on_twice_is_found_with_both_instants() {
    let dir = store("twice");
    proposals(&dir, 1);
    recorded(&dir, &[intent("c0", 0, 10), intent("c0-again", 0, 50)]);

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    match findings.first() {
        Some(Finding::ProposalActedOnTwice { from_position, first_at_ns, again_at_ns }) => {
            assert_eq!((*from_position, *first_at_ns, *again_at_ns), (0, 10, 50));
        }
        other => panic!("a proposal acted on twice must be found: {other:?}"),
    }
}

#[test]
fn a_proposal_refused_and_then_acted_on_is_the_same_finding() {
    let dir = store("refused-then");
    proposals(&dir, 1);
    recorded(
        &dir,
        &[
            ControlEvent::Refused {
                decision_id: "d0".into(),
                reason: bijective_control_engine::wire::RefusalReason::LimitsUnrecovered,
                from_position: 0,
                at_ns: 10,
            },
            intent("c0", 0, 50),
        ],
    );

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    assert!(matches!(findings.first(), Some(Finding::ProposalActedOnTwice { .. })), "{findings:?}");
}

#[test]
fn an_intent_naming_a_proposal_that_does_not_exist_is_found() {
    let dir = store("no-proposal");
    proposals(&dir, 1);
    recorded(&dir, &[intent("c0", 7, 10)]);

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    match findings.first() {
        Some(Finding::ActNamesNoProposal { from_position, proposals_held }) => {
            assert_eq!((*from_position, *proposals_held), (7, 1));
        }
        other => panic!("an intent naming nothing must be found: {other:?}"),
    }
}

#[test]
fn an_unreadable_entry_stops_the_audit() {
    let dir = store("unreadable");
    proposals(&dir, 1);
    let mut w = StreamWriter::open(&dir, CONTROL).expect("control");
    w.append(b"not a control event").expect("append");
    drop(w);

    assert!(matches!(audit(&dir, CONTROL, PROPOSALS), Err(AuditError::Undecodable { at: 0 })));
}

#[test]
fn a_refusal_naming_a_proposal_that_does_not_exist_is_found() {
    let dir = store("refused-nothing");
    proposals(&dir, 1);
    recorded(
        &dir,
        &[ControlEvent::Refused {
            decision_id: "d0".into(),
            reason: bijective_control_engine::wire::RefusalReason::LimitsUnrecovered,
            from_position: 5,
            at_ns: 10,
        }],
    );

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    assert!(
        matches!(findings.first(), Some(Finding::ActNamesNoProposal { from_position: 5, .. })),
        "{findings:?}"
    );
}

#[test]
fn a_held_proposal_and_its_releases_are_one_act() {
    let dir = store("held");
    proposals(&dir, 1);
    let child = bijective_model::Proposal {
        client_id: "d-c0".into(),
        desk: "alpha".into(),
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        side: bijective_model::Side::Sell,
        quantity_scaled: 2,
        price_scaled: Some(2_654_500),
        cap_scaled: None,
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: true,
        parent: Some("entry".into()),
        oco: None,
    };
    let held = ControlEvent::Held { proposal: child, from_position: 0, at_ns: 5 };
    let mut more = intent("c0-more", 0, 20);
    if let ControlEvent::Intent { decision_id, .. } = &mut more {
        *decision_id = "d-c0".into();
    }
    recorded(&dir, &[held, intent("c0", 0, 10), more]);

    let (_, findings) = audit(&dir, CONTROL, PROPOSALS).expect("audit");

    assert!(findings.is_empty(), "{findings:?}");
}
