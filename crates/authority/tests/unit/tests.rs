// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Access control tests.

use super::*;

const ROLES: [Role; 3] = [Role::Observer, Role::Operator, Role::RiskOfficer];
const ACTIONS: [Action; 6] = [
    Action::Halt,
    Action::Reduce,
    Action::Resume,
    Action::ChangeLimit,
    Action::AcknowledgeAlarm,
    Action::ApproveRecommendation,
];

/// Stopping is available to every role, including the floor.
/// A platform where the person who notices cannot stop it keeps trading while someone finds who can.
#[test]
fn every_role_may_stop_the_platform() {
    for role in ROLES {
        assert!(decide(role, Action::Halt).is_permitted(), "{role:?}");
        assert!(decide(role, Action::Reduce).is_permitted(), "{role:?}");
    }
}

/// The floor cannot resume. It is the one action that relaxes.
#[test]
fn an_observer_cannot_resume_trading() {
    let refusal = decide(Role::Observer, Action::Resume);

    assert_eq!(refusal, Decision::Refused { role: Role::Observer, action: Action::Resume });
    assert_eq!(refusal.to_string(), "observer may not resume");
}

/// Only the role that owns what may be risked changes it.
#[test]
fn only_the_risk_officer_changes_a_limit() {
    assert!(!decide(Role::Observer, Action::ChangeLimit).is_permitted());
    assert!(!decide(Role::Operator, Action::ChangeLimit).is_permitted());
    assert!(decide(Role::RiskOfficer, Action::ChangeLimit).is_permitted());
}

/// The same request, the same role, the same decision.
/// A decision that cannot be reproduced cannot be explained.
#[test]
fn the_same_pair_decides_the_same_way_every_time() {
    for role in ROLES {
        for action in ACTIONS {
            let first = decide(role, action);
            for _ in 0..8 {
                assert_eq!(decide(role, action), first, "{role:?}/{action:?}");
            }
        }
    }
}

/// A refusal names the role and the action, so the person is told what they may not do.
#[test]
fn a_refusal_says_who_and_what() {
    for role in ROLES {
        for action in ACTIONS {
            if let Decision::Refused { .. } = decide(role, action) {
                let said = decide(role, action).to_string();
                assert!(said.contains(role.as_str()), "{said}");
                assert!(said.contains(action.as_str()), "{said}");
            }
        }
    }
}

/// A role name round-trips, so what a record holds reads back as what was decided.
#[test]
fn a_role_name_round_trips() {
    for role in ROLES {
        assert_eq!(Role::named(role.as_str()), Some(role));
    }
    assert_eq!(Role::named("administrator"), None, "an unknown role is not a role");
}

/// The floor is the floor: no action an observer may take is one that opens anything.
#[test]
fn nothing_the_floor_may_do_relaxes_the_platform() {
    let relaxing = [Action::Resume, Action::ChangeLimit, Action::ApproveRecommendation];

    for action in relaxing {
        assert!(
            !decide(Role::Observer, action).is_permitted(),
            "an observer must not {}",
            action.as_str()
        );
    }
}

/// Every action reads back from its own name, and no two share one.
#[test]
fn every_action_is_named_once_and_reads_back() {
    for action in Action::ALL {
        assert_eq!(Action::named(action.as_str()), Some(action));
    }
    let mut names: Vec<&str> = Action::ALL.iter().map(|a| a.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), Action::ALL.len());
    assert_eq!(Action::named("administer"), None);
}
