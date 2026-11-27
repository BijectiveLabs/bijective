// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Position adjustments: only a risk officer's, only with a reason, once per command, and what
//! lets a venue that held a position the platform did not place reconcile and resume.

use bijective_authority::Role;
use bijective_event_store::store::StreamWriter;
use bijective_lifecycle::{PositionEvidence, PositionReport, VenueEvidence, Verdict};
use bijective_limits::SafeState;
use bijective_model::{Side, VenueSymbol};
use bijective_serialization::signal::{Command, Instruction, PositionAdjustment};

use crate::path::OrderPath;

fn root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-adjust-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn open(root: &std::path::Path) -> OrderPath {
    let _ = StreamWriter::open(root, "control");
    OrderPath::open(root, "control", "limits").expect("open")
}

fn adjust(id: &str, role: Role, quantity: i128, reason: &str) -> Command {
    Command {
        id: id.into(),
        actor: "risk-desk".into(),
        role,
        instruction: Instruction::AdjustPosition {
            venue: "venue-a".into(),
            adjustment: PositionAdjustment {
                instrument: "aaa".into(),
                side: Side::Buy,
                quantity_scaled: quantity,
                price_scaled: 100,
                reason: reason.into(),
                owner: None,
            },
        },
        at_ns: 1,
    }
}

fn command(instruction: Instruction, role: Role) -> Command {
    Command { id: "cmd-r".into(), actor: "duty".into(), role, instruction, at_ns: 2 }
}

/// The venue reports a long of `quantity` and no orders or fills.
fn holding(quantity: i128) -> VenueEvidence {
    VenueEvidence {
        charges: Vec::new(),
        orders: vec![],
        fills: vec![],
        positions: PositionEvidence::Reported(vec![PositionReport {
            instrument: VenueSymbol::new("aaa"),
            quantity_scaled: quantity,
        }]),
        history_from_ns: None,
        complete: true,
    }
}

fn held(path: &OrderPath) -> i128 {
    path.ledger_for_tests("venue-a")
        .position(&VenueSymbol::new("aaa"))
        .map_or(0, bijective_lifecycle::Position::quantity_scaled)
}

#[test]
fn a_position_held_before_the_platform_is_adjusted_in_and_then_reconciles_and_resumes() {
    let r = root("accepted");
    let mut path = open(&r);
    assert_eq!(
        path.reconcile_venue("venue-a", &holding(15), 1).unwrap().verdict,
        Verdict::Diverged
    );
    let resume = command(Instruction::Resume, Role::Operator);
    assert_eq!(path.carry_out(&resume, None, 0, 2).unwrap(), SafeState::Halted);

    path.carry_out(&adjust("cmd-a", Role::RiskOfficer, 15, "held before the platform"), None, 1, 3)
        .unwrap();
    drop(path);
    let mut path = open(&r);
    assert_eq!(held(&path), 15, "the adjustment survives a restart");
    assert_eq!(path.reconcile_venue("venue-a", &holding(15), 4).unwrap().verdict, Verdict::Aligned);
    assert_eq!(path.carry_out(&resume, None, 2, 5).unwrap(), SafeState::Active);
}

#[test]
fn an_operator_may_not_adjust_and_nothing_moves() {
    let r = root("operator");
    let mut path = open(&r);
    path.carry_out(&adjust("cmd-a", Role::Operator, 15, "held before"), None, 0, 1).unwrap();
    assert_eq!(held(&path), 0);
    drop(path);
    assert_eq!(held(&open(&r)), 0);
}

#[test]
fn an_adjustment_without_a_reason_or_quantity_is_refused() {
    let r = root("unreadable");
    let mut path = open(&r);
    path.carry_out(&adjust("cmd-a", Role::RiskOfficer, 15, "  "), None, 0, 1).unwrap();
    path.carry_out(&adjust("cmd-b", Role::RiskOfficer, 0, "held before"), None, 1, 1).unwrap();
    assert_eq!(held(&path), 0);
}

#[test]
fn one_command_issued_twice_moves_the_position_once() {
    let r = root("twice");
    let mut path = open(&r);
    let once = adjust("cmd-a", Role::RiskOfficer, 15, "held before the platform");
    path.carry_out(&once, None, 0, 1).unwrap();
    path.carry_out(&once, None, 1, 1).unwrap();
    assert_eq!(held(&path), 15);
    drop(path);
    assert_eq!(held(&open(&r)), 15);
}
