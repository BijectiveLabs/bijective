// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the strategies are told of their proposals, and that the telling survives a crash.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bijective_event_store::store::{StreamWriter, read_range};
use bijective_model::{OrderRequest, Side};
use bijective_serialization::signal::feedback::{Outcome, decode as decode_feedback, stream_for};
use bijective_venue::normalise::VenueOutcome;

use super::*;
use crate::wire::{ControlEvent, RefusalReason, encode};

fn intent(client_id: &str, from_position: u64) -> ControlEvent {
    ControlEvent::Intent {
        decision_id: format!("dec-{client_id}"),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: client_id.into(),
            instrument: "AAA".into(),
            side: Side::Buy,
            quantity_scaled: 10,
            price_scaled: Some(100),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position,
        at_ns: 1,
    }
}

fn filled(client_id: &str) -> ControlEvent {
    ControlEvent::Filled {
        client_id: client_id.into(),
        fill: bijective_lifecycle::Fill {
            fill_id: "f-1".into(),
            venue_order_id: Some("v".into()),
            side: Side::Buy,
            price_scaled: 99,
            quantity_scaled: 4,
            fee: None,
            liquidity: bijective_lifecycle::Liquidity::Maker,
            exchange_time: bijective_model::market::ExchangeTime::NotPublished,
            received_at_ns: 3,
        },
        at_ns: 3,
    }
}

fn refused(from_position: u64) -> ControlEvent {
    ControlEvent::Refused {
        decision_id: "dec-r".into(),
        reason: RefusalReason::Unreadable,
        from_position,
        at_ns: 2,
    }
}

#[test]
fn each_event_about_an_order_names_the_proposal_it_came_from() {
    let mut positions = BTreeMap::new();
    let submitted = mirrored(&intent("c-1", 7), &mut positions).expect("an intent is told");
    assert_eq!(
        (submitted.0, submitted.1.as_deref(), &submitted.2),
        (7, Some("dec-c-1"), &Outcome::Submitted),
        "the strategy is told the identifier it proposed under, not the one the venue was sent"
    );

    let accepted = ControlEvent::Settled {
        client_id: "c-1".into(),
        outcome: VenueOutcome::Accepted,
        at_ns: 2,
    };
    assert_eq!(mirrored(&accepted, &mut positions).expect("told").2, Outcome::Accepted);
    let fill = mirrored(&filled("c-1"), &mut positions).expect("told");
    assert_eq!(fill.1.as_deref(), Some("dec-c-1"));
    assert_eq!(
        (fill.0, fill.2),
        (7, Outcome::Filled { fill_id: "f-1".into(), quantity_scaled: 4, price_scaled: 99 })
    );

    let refusal = mirrored(&refused(9), &mut positions).expect("a refusal is told");
    assert_eq!((refusal.0, refusal.1, refusal.2), (9, None, Outcome::Refused));

    // An order no intent named belongs to no proposal this engine can name.
    assert_eq!(mirrored(&filled("stranger"), &mut positions), None);
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-feedback-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn told(root: &std::path::Path) -> Vec<(u64, Outcome)> {
    read_range(root, &stream_for("control"), 0, u64::MAX)
        .unwrap_or_default()
        .iter()
        .map(|e| decode_feedback(&e.payload).expect("a record"))
        .map(|f| (f.control_position, f.outcome))
        .collect()
}

/// A crash between the control append and the mirror's leaves the mirror behind. The next open
/// completes it, once.
#[test]
fn opening_completes_a_mirror_left_behind_and_never_repeats_one() {
    let root = scratch("complete");
    {
        let mut control = StreamWriter::open(&root, "control").expect("control");
        for event in [intent("c-1", 0), refused(1), filled("c-1")] {
            control.append(&encode(&event).expect("encode")).expect("append");
        }
    }
    drop(Mirror::open(&root, "control").expect("open"));
    assert_eq!(told(&root).iter().map(|t| t.0).collect::<Vec<_>>(), [0, 1, 2]);

    drop(Mirror::open(&root, "control").expect("reopen"));
    assert_eq!(told(&root).len(), 3, "a second open repeated what was told");

    {
        let mut control = StreamWriter::open(&root, "control").expect("control");
        control.append(&encode(&refused(5)).expect("encode")).expect("append");
    }
    drop(Mirror::open(&root, "control").expect("reopen"));
    assert_eq!(told(&root).last(), Some(&(3, Outcome::Refused)));
    assert_eq!(told(&root).len(), 4);
}
