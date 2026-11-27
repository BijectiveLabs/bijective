// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The execution cost of an order against its decision's book, exact, positive a cost.

use bijective_control_engine::wire::{CapturedBook, ControlEvent};
use bijective_lifecycle::{Fill, Liquidity};
use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side, VenueSymbol};

use super::*;
use crate::panel::{Entry, Panel};
use crate::table::Cell;

const DECIDED_NS: i128 = 5;

fn scales() -> ControlEvent {
    ControlEvent::InstrumentScales {
        venue: "venue-a".into(),
        instrument: VenueSymbol::new("aaa"),
        price_scale: 2,
        quantity_scale: 3,
        at_ns: 1,
        identity: Some("venue-a.aaa".into()),
        quote_currency: Some("usd".into()),
    }
}

fn captured(book: Option<CapturedBook>) -> ControlEvent {
    let detail = if book.is_some() { String::new() } else { "SnapshotAfterDecision".into() };
    ControlEvent::Captured {
        correlation_id: "d-1".into(),
        instrument: VenueSymbol::new("aaa"),
        decided_at_ns: DECIDED_NS,
        book,
        detail,
        from_position: 0,
        at_ns: 6,
    }
}

/// Bid 1.99, ask 2.01: the mid is 2.00.
fn book() -> CapturedBook {
    CapturedBook {
        best_bid_ticks: 199,
        best_bid_qty_lots: 10,
        best_ask_ticks: 201,
        best_ask_qty_lots: 10,
        observed_at_ns: 4,
        capture_lag_ns: 1,
    }
}

fn intent(side: Side) -> ControlEvent {
    ControlEvent::Intent {
        decision_id: "d-1".into(),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: "bl-1".into(),
            instrument: VenueSymbol::new("aaa"),
            side,
            quantity_scaled: 1_000,
            price_scaled: Some(205),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position: 0,
        at_ns: 7,
    }
}

fn filled(
    id: &str,
    side: Side,
    price_scaled: i128,
    quantity_scaled: i128,
    at: i128,
) -> ControlEvent {
    let fill = Fill {
        fill_id: id.into(),
        venue_order_id: None,
        side,
        price_scaled,
        quantity_scaled,
        fee: None,
        liquidity: Liquidity::Taker,
        exchange_time: ExchangeTime::NotPublished,
        received_at_ns: at,
    };
    ControlEvent::Filled { client_id: "bl-1".into(), fill, at_ns: at + 1 }
}

fn cost_row(events: &[ControlEvent]) -> Vec<Cell> {
    let mut panel = ExecutionPanel::default();
    for event in events {
        panel.apply(&Entry::Control(event));
    }
    let table = panel.rows().expect("a table");
    assert_eq!(table.rows.len(), 1);
    table.rows[0].clone()
}

/// 0.4 at 2.00 and 0.6 at 2.02 average 2.012 against a mid of 2.00: 0.012 paid, 0.010 of it
/// crossing to the ask, 60 basis points.
#[test]
fn a_buy_pays_above_the_mid_and_the_cost_splits_into_crossing_and_residual() {
    let events = [
        scales(),
        captured(Some(book())),
        intent(Side::Buy),
        filled("t-1", Side::Buy, 200, 400, 16),
        filled("t-2", Side::Buy, 202, 600, 20),
    ];
    let row = cost_row(&events);
    assert_eq!(row[6], Cell::text("1.99"));
    assert_eq!(row[7], Cell::text("2.01"));
    assert_eq!(row[10], Cell::Integer(2));
    assert_eq!(row[11], Cell::text("1.000"));
    assert_eq!(row[12], Cell::text("2.012000"));
    assert_eq!(row[13], Cell::text("0.012000"));
    assert_eq!(row[14], Cell::text("60.00"));
    assert_eq!(row[15], Cell::text("0.010000"));
    assert_eq!(row[16], Cell::text("0.002000"));
    assert_eq!(row[17], Cell::Integer(16 - DECIDED_NS));
}

/// A sell below the mid is a cost too. One above it is a negative cost.
#[test]
fn a_sell_below_the_mid_is_a_cost_and_above_it_a_gain() {
    let below = cost_row(&[
        scales(),
        captured(Some(book())),
        intent(Side::Sell),
        filled("t-1", Side::Sell, 198, 400, 16),
    ]);
    assert_eq!(below[13], Cell::text("0.008000"), "0.02 below the mid on 0.4");
    assert_eq!(below[14], Cell::text("100.00"));
    let above = cost_row(&[
        scales(),
        captured(Some(book())),
        intent(Side::Sell),
        filled("t-1", Side::Sell, 201, 1_000, 16),
    ]);
    assert_eq!(above[13], Cell::text("-0.010000"));
    assert_eq!(above[15], Cell::text("0.010000"), "what crossing to the bid would have cost");
    assert_eq!(above[16], Cell::text("-0.020000"), "what filling at the ask earned beyond it");
}

/// Without a book there is no benchmark: the cost is absent, never zero, and the reason is shown.
#[test]
fn without_a_decision_book_the_cost_is_absent_and_says_why() {
    let row = cost_row(&[
        scales(),
        captured(None),
        intent(Side::Buy),
        filled("t-1", Side::Buy, 200, 400, 16),
    ]);
    assert_eq!(row[9], Cell::text("SnapshotAfterDecision"));
    assert_eq!(row[12], Cell::text("2.000000"), "the fills are still averaged");
    assert_eq!(row[13], Cell::Absent);
    assert_eq!(row[14], Cell::Absent);
    let unfilled = cost_row(&[scales(), captured(Some(book())), intent(Side::Buy)]);
    assert_eq!(unfilled[10], Cell::Integer(0));
    assert_eq!(unfilled[11], Cell::Absent, "nothing filled is not a quantity of zero");
    assert_eq!(unfilled[13], Cell::Absent);
}
