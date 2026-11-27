// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Which resting orders a non-active node withdraws, and which it leaves as exits.

use std::collections::{BTreeMap, BTreeSet};

use bijective_event_store::projection::Projection;
use bijective_limits::SafeState;
use bijective_model::{OrderRequest, Side};
use bijective_venue::normalise::VenueOutcome;
use bijective_venue::profile::ReduceOnly;

use super::resting_to_withdraw;
use crate::orders::Orders;
use crate::wire::{ControlEvent, EvidenceRead};

fn accepted(venue: &str, client_id: &str, reduce_only: bool) -> [ControlEvent; 2] {
    [
        ControlEvent::Intent {
            decision_id: format!("dec-{client_id}"),
            venue: Some(venue.into()),
            owner: None,
            request: OrderRequest {
                client_id: client_id.into(),
                instrument: "AAA".into(),
                side: Side::Buy,
                quantity_scaled: 10,
                price_scaled: Some(100),
                reduce_only,
                terms: Default::default(),
            },
            from_position: 0,
            at_ns: 1,
        },
        ControlEvent::Settled {
            client_id: client_id.into(),
            outcome: VenueOutcome::Accepted,
            at_ns: 2,
        },
    ]
}

fn fold(events: impl IntoIterator<Item = ControlEvent>) -> Orders {
    let mut orders = Orders::default();
    for e in events {
        orders.apply(&e).expect("applied");
    }
    orders
}

fn guarantees(entries: &[(&str, ReduceOnly)]) -> BTreeMap<String, ReduceOnly> {
    entries.iter().map(|(v, g)| ((*v).to_string(), *g)).collect()
}

fn ids(
    orders: &Orders,
    state: SafeState,
    guarantees: &BTreeMap<String, ReduceOnly>,
) -> Vec<String> {
    resting_to_withdraw(orders, state, guarantees, &BTreeSet::new(), 0)
        .into_iter()
        .map(|r| r.client_id)
        .collect()
}

#[test]
fn an_active_node_withdraws_nothing() {
    let orders = fold(accepted("a", "x", false));
    assert!(
        ids(&orders, SafeState::Active, &guarantees(&[("a", ReduceOnly::PlacementPrecondition)]))
            .is_empty()
    );
}

#[test]
fn a_halt_withdraws_what_can_add_exposure_and_leaves_the_exits() {
    let orders = fold(accepted("a", "entry", false).into_iter().chain(accepted("a", "stop", true)));
    let enforced = guarantees(&[("a", ReduceOnly::PlacementPrecondition)]);
    for state in [SafeState::Halted, SafeState::Reducing] {
        assert_eq!(ids(&orders, state, &enforced), vec!["entry".to_string()]);
    }
}

#[test]
fn a_reduce_only_order_the_venue_does_not_enforce_is_not_an_exit() {
    let orders = fold(accepted("a", "stop", true));
    let weak = guarantees(&[("a", ReduceOnly::Unsupported)]);
    assert_eq!(ids(&orders, SafeState::Halted, &weak), vec!["stop".to_string()]);
    let clamp = guarantees(&[("a", ReduceOnly::FillTimeClamp)]);
    assert!(ids(&orders, SafeState::Halted, &clamp).is_empty());
    assert_eq!(
        ids(&orders, SafeState::Halted, &BTreeMap::new()),
        vec!["stop".to_string()],
        "a venue with no declared guarantee vouches for nothing"
    );
}

#[test]
fn a_venue_that_cannot_be_read_or_has_diverged_keeps_no_exits() {
    let enforced = guarantees(&[("a", ReduceOnly::PlacementPrecondition)]);
    let unread = ControlEvent::ReconciliationCompleted {
        venue: "a".into(),
        verdict: bijective_lifecycle::Verdict::Unverified,
        findings: Vec::new(),
        evidence: EvidenceRead::Failed,
        at_ns: 3,
    };
    let orders = fold(accepted("a", "stop", true).into_iter().chain([unread]));
    assert_eq!(ids(&orders, SafeState::Halted, &enforced), vec!["stop".to_string()]);

    let diverged = ControlEvent::ReconciliationCompleted {
        venue: "a".into(),
        verdict: bijective_lifecycle::Verdict::Diverged,
        findings: vec!["a position the record does not hold".into()],
        evidence: EvidenceRead::Read,
        at_ns: 3,
    };
    let orders = fold(accepted("a", "stop", true).into_iter().chain([diverged]));
    assert_eq!(ids(&orders, SafeState::Halted, &enforced), vec!["stop".to_string()]);
}

#[test]
fn only_orders_that_are_working_are_withdrawn_and_only_once() {
    let mut events: Vec<ControlEvent> = accepted("a", "live", false).into();
    events.extend(accepted("a", "asked", false));
    events.push(ControlEvent::Withdrawn {
        client_id: "asked".into(),
        instrument: "AAA".into(),
        outcome: VenueOutcome::Accepted,
        at_ns: 3,
    });
    let orders = fold(events);
    let enforced = guarantees(&[("a", ReduceOnly::PlacementPrecondition)]);
    assert_eq!(
        ids(&orders, SafeState::Halted, &enforced),
        vec!["live".to_string()],
        "a cancel already asked for is not asked again"
    );
}

fn cancel_orders(detail: &str) -> ControlEvent {
    ControlEvent::CommandApplied {
        command_id: "cmd-0".into(),
        actor: "ana".into(),
        role: bijective_authority::Role::Operator,
        action: bijective_authority::Action::CancelOrders,
        state: SafeState::Active,
        detail: detail.into(),
        from_position: 0,
        at_ns: 3,
    }
}

/// Exits included and while active: it is an order to cancel what was working, not a halt.
#[test]
fn a_cancel_orders_command_withdraws_every_order_working_when_it_was_carried_out() {
    let before = accepted("a", "entry", false).into_iter().chain(accepted("a", "stop", true));
    let after = accepted("a", "later", false);
    let orders = fold(before.chain([cancel_orders("")]).chain(after));
    let enforced = guarantees(&[("a", ReduceOnly::PlacementPrecondition)]);
    assert_eq!(ids(&orders, SafeState::Active, &enforced), vec!["entry", "stop"]);
}

#[test]
fn a_refused_cancel_orders_command_names_no_order() {
    let orders = fold(accepted("a", "entry", false).into_iter().chain([cancel_orders("refused")]));
    let enforced = guarantees(&[("a", ReduceOnly::PlacementPrecondition)]);
    assert!(ids(&orders, SafeState::Active, &enforced).is_empty());
}

#[test]
fn an_active_node_withdraws_an_order_whose_expiry_has_passed() {
    let mut events = accepted("a", "dated", false);
    if let ControlEvent::Intent { request, .. } = &mut events[0] {
        request.terms.expire_at_ns = Some(100);
    }
    let orders = fold(events);
    let enforced = guarantees(&[("a", ReduceOnly::PlacementPrecondition)]);
    let at = |now| {
        resting_to_withdraw(&orders, SafeState::Active, &enforced, &BTreeSet::new(), now)
            .into_iter()
            .map(|r| r.client_id)
            .collect::<Vec<_>>()
    };
    assert!(at(99).is_empty(), "before its expiry it works");
    assert_eq!(at(100), vec!["dated".to_string()]);
}

fn flatten_command(command_id: &str) -> ControlEvent {
    ControlEvent::CommandApplied {
        command_id: command_id.into(),
        actor: "ana".into(),
        role: bijective_authority::Role::Operator,
        action: bijective_authority::Action::Flatten,
        state: SafeState::Reducing,
        detail: String::new(),
        from_position: 0,
        at_ns: 3,
    }
}

/// The order a flatten placed to close a position, under the flatten's decision identifier.
fn closing_order() -> [ControlEvent; 2] {
    let mut events = accepted("a", "close", true);
    if let ControlEvent::Intent { decision_id, .. } = &mut events[0] {
        *decision_id = crate::wire::flatten_decision_id("cmd-1");
    }
    events
}

/// Where the venue does not enforce reduce-only, the sweep withdrew the flatten's own market order
/// while it waited for its fill, and the flatten then placed it again (a paper equity venue,
/// 2026-11-23).
#[test]
fn the_flatten_in_force_keeps_its_closing_order_where_reduce_only_is_not_enforced() {
    let before = accepted("a", "stop", true).into_iter().chain([flatten_command("cmd-1")]);
    let orders = fold(before.chain(closing_order()));
    let weak = guarantees(&[("a", ReduceOnly::Unsupported)]);
    assert_eq!(ids(&orders, SafeState::Reducing, &weak), vec!["stop".to_string()]);
}

#[test]
fn a_closing_order_is_withdrawn_once_its_venue_is_untrusted_or_the_flatten_is_over() {
    let weak = guarantees(&[("a", ReduceOnly::Unsupported)]);
    let diverged = ControlEvent::ReconciliationCompleted {
        venue: "a".into(),
        verdict: bijective_lifecycle::Verdict::Diverged,
        findings: vec!["a position the record does not hold".into()],
        evidence: EvidenceRead::Read,
        at_ns: 4,
    };
    let orders =
        fold([flatten_command("cmd-1")].into_iter().chain(closing_order()).chain([diverged]));
    assert_eq!(ids(&orders, SafeState::Reducing, &weak), vec!["close".to_string()]);

    let other = flatten_command("cmd-2");
    let orders = fold([flatten_command("cmd-1")].into_iter().chain(closing_order()).chain([other]));
    assert_eq!(
        ids(&orders, SafeState::Reducing, &weak),
        vec!["close".to_string()],
        "a later flatten names its own orders, and this one was ordered cancelled"
    );
}
