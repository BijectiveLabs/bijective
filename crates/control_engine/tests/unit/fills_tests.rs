// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Fills on the control stream: recorded once, only for this platform's orders, only when the
//! lifecycle accepts them, and rebuilt into the order and its position on restart.

use bijective_lifecycle::{Fee, Fill, Liquidity, OrderState};
use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::VenueOutcome;

use crate::path::{FillRecorded, OrderPath};
use crate::wire::{ControlEvent, decode, encode};

fn root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-fills-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn intent_then_accepted(root: &std::path::Path) {
    let request = OrderRequest {
        client_id: "c-1".into(),
        instrument: "aaa".into(),
        side: Side::Buy,
        quantity_scaled: 10,
        price_scaled: Some(100),
        reduce_only: false,
        terms: Default::default(),
    };
    let mut w = bijective_event_store::store::StreamWriter::open(root, "control").unwrap();
    let intent = ControlEvent::Intent {
        decision_id: "d-1".into(),
        venue: Some("venue-a".into()),
        owner: None,
        request,
        from_position: 0,
        at_ns: 1,
    };
    let settled = ControlEvent::Settled {
        client_id: "c-1".into(),
        outcome: VenueOutcome::Accepted,
        at_ns: 2,
    };
    w.append(&encode(&intent).unwrap()).unwrap();
    w.append(&encode(&settled).unwrap()).unwrap();
}

fn fill(id: &str, quantity: i128) -> Fill {
    Fill {
        fill_id: id.into(),
        venue_order_id: Some("v-1".into()),
        side: Side::Buy,
        price_scaled: 100,
        quantity_scaled: quantity,
        fee: Some(Fee { asset: "USDT".into(), amount_scaled: 55, scale: 12 }),
        liquidity: Liquidity::Maker,
        exchange_time: ExchangeTime::At { at_ns: 3 },
        received_at_ns: 4,
    }
}

fn open(root: &std::path::Path) -> OrderPath {
    OrderPath::open(root, "control", "limits").expect("open")
}

#[test]
fn a_fill_event_round_trips_with_every_optional_field_both_ways() {
    let mut bare = fill("f-0", 1);
    bare.venue_order_id = None;
    bare.fee = None;
    bare.exchange_time = ExchangeTime::NotPublished;
    bare.liquidity = Liquidity::NotPublished;
    for f in [fill("f-1", 2), bare] {
        let event = ControlEvent::Filled { client_id: "c-1".into(), fill: f, at_ns: 9 };
        let bytes = encode(&event).unwrap();
        assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
        assert_eq!(decode(&bytes).unwrap(), event);
    }
}

#[test]
fn a_recorded_fill_moves_the_order_and_its_position_and_survives_a_restart() {
    let r = root("restart");
    intent_then_accepted(&r);
    {
        let mut path = open(&r);
        assert!(matches!(
            path.record_fill("c-1", &fill("f-1", 4), 5).unwrap(),
            FillRecorded::Recorded { .. }
        ));
    }
    let path = open(&r);
    let (summary, findings) = path.orders();
    assert_eq!((summary.may_execute, findings), (1, 0));
    let replayed = path.ledger_for_tests("venue-a");
    let order = replayed.order("c-1").unwrap();
    assert_eq!((order.state, order.filled_quantity_scaled), (OrderState::PartiallyFilled, 4));
    assert_eq!(replayed.position(&VenueSymbol::new("aaa")).unwrap().quantity_scaled(), 4);
}

#[test]
fn a_fill_delivered_twice_is_written_once() {
    let r = root("twice");
    intent_then_accepted(&r);
    let mut path = open(&r);
    assert!(matches!(
        path.record_fill("c-1", &fill("f-1", 4), 5).unwrap(),
        FillRecorded::Recorded { .. }
    ));
    let mut again = fill("f-1", 4);
    again.received_at_ns = 99;
    assert_eq!(path.record_fill("c-1", &again, 6).unwrap(), FillRecorded::Duplicate);
}

#[test]
fn a_fill_for_an_order_not_placed_here_or_one_the_lifecycle_refuses_is_not_written() {
    let r = root("refused");
    intent_then_accepted(&r);
    let mut path = open(&r);
    assert_eq!(
        path.record_fill("someone-else", &fill("f-1", 1), 5).unwrap(),
        FillRecorded::NotOurs
    );
    assert!(matches!(
        path.record_fill("c-1", &fill("f-2", 11), 5).unwrap(),
        FillRecorded::Refused { .. }
    ));
    let (summary, _) = path.orders();
    assert_eq!(summary.positions_open, 0, "nothing was written");
}
