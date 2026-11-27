// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Linked proposals through the order path (decision 0013): held, released on the parent's
//! fills, once across a restart, and a filled leg's peer withdrawn by the sweep.

use super::*;
use crate::loop_::ProposalWindow;

use bijective_lifecycle::{Fill, Liquidity};
use bijective_model::market::ExchangeTime;

fn entry() -> Proposal {
    Proposal { client_id: "entry".into(), price_scaled: Some(200), ..signal("entry", 10) }
}

fn take_profit() -> Proposal {
    Proposal {
        side: Side::Sell,
        price_scaled: Some(205),
        reduce_only: true,
        parent: Some("entry".into()),
        oco: Some("bracket".into()),
        ..signal("tp", 10)
    }
}

fn stop_loss() -> Proposal {
    let mut stop = Proposal { price_scaled: None, cap_scaled: Some(190), ..take_profit() };
    stop.client_id = "sl".into();
    stop.terms.stop_price_scaled = Some(190);
    stop
}

fn judged<'a>(
    mandates: &'a MandateBook,
    evaluation: &'a Evaluation,
    book: &'a SnapshotBuffer,
    marks: &'a BTreeMap<InstrumentId, bijective_risk::valuation::Mark>,
    resolved: &'a BTreeSet<InstrumentId>,
) -> Judgement<'a> {
    Judgement {
        resolved,
        mandates,
        evaluation,
        position_held: PositionHeld::No,
        book,
        staleness_bound_ns: 5_000_000_000,
        marks,
    }
}

/// One pass over the proposal stream against `reachable`, with the mark at `mark`.
async fn pass(
    r: &std::path::Path,
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
    mark: i128,
) {
    let marks = BTreeMap::from([(
        InstrumentId::new("aaa"),
        bijective_risk::valuation::Mark { price_scaled: mark, observed_at_ns: 1_000 },
    )]);
    let (mandates, evaluation, book) = (mandate_book(1_000_000_000), clear(), empty_book());
    let resolved = BTreeSet::new();
    let judgement = judged(&mandates, &evaluation, &book, &marks, &resolved);
    drive(path, reachable, &clock(), r, ProposalWindow::whole("proposals"), &judgement)
        .await
        .expect("the loop works");
}

/// The client identifier and quantity of every intent `decision` placed.
fn placed_for(r: &std::path::Path, decision: &str) -> Vec<(String, i128)> {
    journal(r)
        .into_iter()
        .filter_map(|e| match e {
            ControlEvent::Intent { decision_id, request, .. } if decision_id == decision => {
                Some((request.client_id, request.quantity_scaled))
            }
            _ => None,
        })
        .collect()
}

fn fill(id: &str, side: Side, quantity: i128) -> Fill {
    Fill {
        fill_id: id.into(),
        venue_order_id: None,
        side,
        price_scaled: 200,
        quantity_scaled: quantity,
        fee: None,
        liquidity: Liquidity::Taker,
        exchange_time: ExchangeTime::At { at_ns: 3 },
        received_at_ns: 3,
    }
}

/// `venue` as reachable, its declaration saying it does not enforce reduce-only.
fn unenforced_at<'a>(venue: &'a mut Venue, unenforced: &VenueProfile) -> Reachable<'a> {
    let mut reach = reachable(venue);
    reach.declarations.insert("venue-a".into(), CapabilityDeclaration::from_profile(unenforced));
    reach
}

fn filled(path: &mut OrderPath, client_id: &str, fill: &Fill) {
    let recorded = path.record_fill(client_id, fill, 2).expect("written");
    assert!(matches!(recorded, crate::path::FillRecorded::Recorded { .. }), "{recorded:?}");
}

fn opened(r: &std::path::Path) -> OrderPath {
    let mut path = OrderPath::open(r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);
    path
}

fn refusals(r: &std::path::Path) -> Vec<(String, RefusalReason)> {
    journal(r)
        .into_iter()
        .filter_map(|e| match e {
            ControlEvent::Refused { decision_id, reason, .. } => Some((decision_id, reason)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_bracket_releases_its_legs_on_the_entry_fill_and_once_across_a_restart() {
    let r = root("bracket");
    write_proposals(&r, &[entry(), take_profit(), stop_loss()]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = opened(&r);

    pass(&r, &mut path, &mut reachable(&mut venue), 200).await;
    assert_eq!(venue.submissions, 1, "the legs are held until the entry fills");
    assert_eq!(path.resume_from(), 3, "holding acts on the legs' positions");

    let (entry_order, _) = placed_for(&r, "entry").remove(0);
    filled(&mut path, &entry_order, &fill("f1", Side::Buy, 10));
    pass(&r, &mut path, &mut reachable(&mut venue), 200).await;
    assert_eq!(venue.submissions, 3);
    assert_eq!(placed_for(&r, "tp").iter().map(|p| p.1).collect::<Vec<_>>(), vec![10]);
    assert_eq!(placed_for(&r, "sl").iter().map(|p| p.1).collect::<Vec<_>>(), vec![10]);
    assert!(refusals(&r).is_empty(), "two legs of one group are not counted together");

    drop(path);
    let mut again = opened(&r);
    pass(&r, &mut again, &mut reachable(&mut venue), 200).await;
    assert_eq!(venue.submissions, 3, "a restart releases nothing twice");
}

#[tokio::test]
async fn a_fill_on_one_leg_has_the_sweep_withdraw_the_other() {
    let r = root("oco");
    write_proposals(&r, &[entry(), take_profit(), stop_loss()]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = opened(&r);
    pass(&r, &mut path, &mut reachable(&mut venue), 200).await;
    let (entry_order, _) = placed_for(&r, "entry").remove(0);
    filled(&mut path, &entry_order, &fill("f1", Side::Buy, 10));
    pass(&r, &mut path, &mut reachable(&mut venue), 200).await;

    let (tp_order, _) = placed_for(&r, "tp").remove(0);
    let (sl_order, _) = placed_for(&r, "sl").remove(0);
    filled(&mut path, &tp_order, &fill("f2", Side::Sell, 4));
    let swept =
        crate::withdraw_resting(&mut path, &mut reachable(&mut venue), 4).await.expect("swept");
    let withdrawn: Vec<String> = swept.withdrawn.into_iter().map(|(id, _)| id).collect();
    assert_eq!(withdrawn, vec![sl_order]);
}

#[tokio::test]
async fn a_child_that_does_not_reduce_only_or_names_no_parent_is_refused() {
    let r = root("orphan");
    let opens = Proposal { reduce_only: false, ..take_profit() };
    let orphan =
        Proposal { parent: Some("nobody".into()), client_id: "tp2".into(), ..take_profit() };
    write_proposals(&r, &[entry(), opens, orphan]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = opened(&r);
    pass(&r, &mut path, &mut reachable(&mut venue), 200).await;

    let refused: Vec<String> = refusals(&r).into_iter().map(|(id, _)| id).collect();
    assert_eq!(refused, vec!["tp".to_string(), "tp2".to_string()]);
    assert!(!journal(&r).iter().any(|e| matches!(e, ControlEvent::Held { .. })));
}

#[tokio::test]
async fn a_take_profit_waits_for_its_price_where_the_venue_does_not_enforce_reduce_only() {
    let r = root("touch");
    write_proposals(&r, &[entry(), take_profit(), stop_loss()]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut unenforced = profile();
    unenforced.reduce_only = ReduceOnly::Unsupported;
    venue.declaration = CapabilityDeclaration::from_profile(&unenforced);
    let mut path = opened(&r);

    pass(&r, &mut path, &mut unenforced_at(&mut venue, &unenforced), 200).await;
    let (entry_order, _) = placed_for(&r, "entry").remove(0);
    filled(&mut path, &entry_order, &fill("f1", Side::Buy, 10));
    pass(&r, &mut path, &mut unenforced_at(&mut venue, &unenforced), 200).await;
    assert_eq!(placed_for(&r, "sl").len(), 1, "the stop rests at the venue");
    assert!(placed_for(&r, "tp").is_empty(), "the take-profit waits for its price");

    pass(&r, &mut path, &mut unenforced_at(&mut venue, &unenforced), 205).await;
    assert_eq!(placed_for(&r, "tp").len(), 1, "released when the mark reaches it");
}

/// A proposal whose expiry has passed on this node's clock is refused, never sent (decision 0014).
#[tokio::test]
async fn a_proposal_that_expired_before_admission_is_refused() {
    let r = root("expired");
    let mut late = signal("late", 10);
    late.terms.expire_at_ns = Some(500);
    write_proposals(&r, &[late]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = opened(&r);
    pass(&r, &mut path, &mut reachable(&mut venue), 200).await;
    assert_eq!(venue.submissions, 0);
    assert_eq!(refusals(&r).into_iter().map(|(id, _)| id).collect::<Vec<_>>(), vec!["late"]);
}
