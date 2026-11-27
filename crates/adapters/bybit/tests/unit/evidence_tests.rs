// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Row parsing against the venue's own documented examples (`fixtures/documented/`, taken from the
//! V5 documentation pages cited in `docs/integrations/bybit.md`), and each refusal by the field
//! that causes it.

use bijective_lifecycle::{Liquidity, ReportedStatus};
use bijective_model::Side;
use bijective_model::market::ExchangeTime;
use serde_json::Value;

use super::*;

/// Every symbol the fixtures name, at one pair. A symbol left out would read at the
/// outside-catalogue scale (decision 0007).
fn at(price: u8, quantity: u8) -> bijective_venue::profile::InstrumentScales {
    let pair = bijective_venue::profile::Scales {
        price: bijective_venue::normalise::Scale::new(price).unwrap(),
        quantity: bijective_venue::normalise::Scale::new(quantity).unwrap(),
    };
    bijective_venue::profile::InstrumentScales::uniform_over(
        [
            "AAAUSDT",
            "BTCUSDT",
            "ETHUSDT",
            "ROSEUSDT",
            "BTCUSD",
            "ETHPERP",
            "ETH-30DEC22-1400-C",
            "A",
            "S",
        ],
        pair,
    )
}

const EXECUTIONS: &str = include_str!("../../fixtures/documented/execution_list.json");
const OPEN_ORDERS: &str = include_str!("../../fixtures/documented/open_orders.json");
const POSITIONS: &str = include_str!("../../fixtures/documented/position_list.json");
const STREAM_EXECUTION: &str = include_str!("../../fixtures/documented/stream_execution.json");
const STREAM_ORDER: &str = include_str!("../../fixtures/documented/stream_order.json");

fn scales() -> bijective_venue::profile::InstrumentScales {
    at(2, 3)
}

fn rows(body: &str) -> Vec<Value> {
    read_page(200, body).expect("page").rows
}

#[test]
fn a_documented_execution_row_becomes_a_fill() {
    let row = &rows(EXECUTIONS)[0];
    let Execution::Fill(report) = execution(row, &scales(), 7).expect("read") else {
        panic!("not a fill")
    };
    assert_eq!(
        report.client_id, None,
        "an empty orderLinkId is an order this platform did not name"
    );
    let f = &report.fill;
    assert_eq!(f.fill_id, "e0cbe81d-0f18-5866-9415-cf319b5dab3b");
    assert_eq!(f.venue_order_id.as_deref(), Some("8c065341-7b52-4ca9-ac2c-37e31ac55c94"));
    assert_eq!((f.side, f.price_scaled, f.quantity_scaled), (Side::Buy, 119_015, 100));
    assert_eq!(f.fee.as_ref().map(|fee| fee.amount_scaled), Some(71_409_000_000));
    assert_eq!(f.liquidity, Liquidity::Taker);
    assert_eq!(f.exchange_time, ExchangeTime::from_ms(1_672_282_722_429));
    assert_eq!(f.received_at_ns, 7);
}

#[test]
fn a_documented_stream_execution_reads_the_same_way() {
    let frame: Value = serde_json::from_str(STREAM_EXECUTION).unwrap();
    let row = &frame["data"][0];
    let Execution::Fill(report) = execution(row, &at(1, 3), 1).unwrap() else {
        panic!("not a fill");
    };
    assert_eq!((report.fill.price_scaled, report.fill.quantity_scaled), (959_001, 500));
    assert_eq!(report.fill.fee.as_ref().map(|f| f.asset.as_str()), Some("USDT"));
}

#[test]
fn a_documented_open_order_becomes_a_status_report() {
    let report = order_report(&rows(OPEN_ORDERS)[0], &scales()).expect("read");
    assert_eq!(report.client_id.as_deref(), Some("test-000005"));
    assert_eq!((report.ordered_quantity_scaled, report.filled_quantity_scaled), (100, 0));
    assert_eq!(report.status, ReportedStatus::Working);
    assert_eq!(report.reported_at, ExchangeTime::from_ms(1_684_738_540_561));
}

/// The documented stream example is an option order. This adapter trades linear only.
#[test]
fn a_row_of_another_category_is_refused() {
    let frame: Value = serde_json::from_str(STREAM_ORDER).unwrap();
    let refused = order_report(&frame["data"][0], &scales()).unwrap_err();
    assert_eq!(refused.field, "category");
}

/// The documented position is hedge index 0 and short. Its size is always positive and its side
/// carries the sign.
#[test]
fn a_documented_position_is_signed_by_its_side() {
    let report = position_report(&rows(POSITIONS)[0], &scales()).expect("read");
    assert_eq!((report.instrument.as_str(), report.quantity_scaled), ("BTCUSD", -300_000));
}

#[test]
fn an_empty_position_is_an_explicit_flat_and_hedge_mode_is_refused() {
    let flat: Value =
        serde_json::json!({ "positionIdx": 0, "symbol": "BTCUSDT", "side": "", "size": "0" });
    assert_eq!(position_report(&flat, &scales()).unwrap().quantity_scaled, 0);
    let hedged: Value =
        serde_json::json!({ "positionIdx": 1, "symbol": "BTCUSDT", "side": "Buy", "size": "1" });
    assert_eq!(position_report(&hedged, &scales()).unwrap_err().field, "positionIdx");
    let sideless: Value =
        serde_json::json!({ "positionIdx": 0, "symbol": "BTCUSDT", "side": "", "size": "1" });
    assert_eq!(position_report(&sideless, &scales()).unwrap_err().field, "side");
}

/// The funding row the venue wrote for a long ETHUSDT position at 2026-11-19 16:00 UTC, as read.
const FUNDING_ROW: &str = r#"{"symbol": "ETHUSDT", "orderType": "UNKNOWN", "orderLinkId": "", "orderId": "ea21ce40-03b4-41b1-b0ca-34315f51a4df", "execTime": "1795104000000", "feeCurrency": "USDT", "execFeeV2": "0.00322492", "feeRate": "0.000036", "markPrice": "1781.04", "execPrice": "1781.23", "orderQty": "0", "orderPrice": "0", "execValue": "89.0615", "closedSize": "0", "execType": "Funding", "side": "Buy", "leavesQty": "0", "isMaker": false, "execFee": "0.00322492", "execId": "a904f4ed-553c-422c-8bc9-aa94efa7fe54", "execQty": "0.05", "category": "linear"}"#;

/// Paid funding is a charge at its own sign, so the daily loss counts it like a fee.
#[test]
fn funding_is_a_charge_and_an_unmapped_execution_is_refused() {
    let row: Value = serde_json::from_str(FUNDING_ROW).expect("json");
    let Ok(Execution::Funding(charge)) = execution(&row, &scales(), 1) else {
        panic!("a funding charge");
    };
    assert_eq!(charge.charge_id, "a904f4ed-553c-422c-8bc9-aa94efa7fe54");
    assert_eq!((charge.fee.asset.as_str(), charge.fee.amount_scaled), ("USDT", 3_224_920_000));
    let mut other = rows(EXECUTIONS)[0].clone();
    other["execType"] = Value::from("MovePosition");
    assert_eq!(execution(&other, &scales(), 1).unwrap_err().field, "execType");
}

#[test]
fn a_funding_row_reaches_the_evidence_as_a_charge_not_a_fill() {
    let gathered = Gathered {
        orders: Vec::new(),
        executions: vec![serde_json::from_str(FUNDING_ROW).expect("json")],
        positions: Vec::new(),
    };
    let (evidence, refused) = assemble(&gathered, &[], &scales(), 0, 1);
    assert!(refused.is_empty() && evidence.complete);
    assert!(evidence.fills.is_empty());
    assert_eq!(evidence.charges.len(), 1);
}

/// Excess precision is refused, never rounded.
#[test]
fn a_price_finer_than_the_scale_is_refused() {
    let mut row = rows(EXECUTIONS)[0].clone();
    row["execPrice"] = Value::from("1190.155");
    assert_eq!(execution(&row, &scales(), 1).unwrap_err().field, "execPrice");
}

#[test]
fn every_documented_order_status_is_mapped_and_an_undocumented_one_refused() {
    let mut row = rows(OPEN_ORDERS)[0].clone();
    for (status, want) in [
        ("New", ReportedStatus::Working),
        ("PartiallyFilled", ReportedStatus::Working),
        ("Untriggered", ReportedStatus::Working),
        ("Triggered", ReportedStatus::Working),
        ("Filled", ReportedStatus::Filled),
        ("Cancelled", ReportedStatus::Canceled),
        ("PartiallyFilledCanceled", ReportedStatus::Canceled),
        ("Deactivated", ReportedStatus::Canceled),
        ("Rejected", ReportedStatus::Rejected),
    ] {
        row["orderStatus"] = Value::from(status);
        assert_eq!(order_report(&row, &scales()).unwrap().status, want, "{status}");
    }
    row["orderStatus"] = Value::from("Paused");
    assert_eq!(order_report(&row, &scales()).unwrap_err().field, "orderStatus");
}

#[test]
fn a_page_names_its_cursor_and_a_refused_response_is_not_an_empty_list() {
    assert!(read_page(200, OPEN_ORDERS).unwrap().next_cursor.is_some());
    assert_eq!(
        read_page(200, POSITIONS).unwrap().next_cursor,
        None,
        "an empty cursor is the last page"
    );
    let refused = r#"{"retCode":10003,"retMsg":"API key is invalid.","result":{}}"#;
    assert_eq!(read_page(200, refused).unwrap_err().field, "retCode");
    assert_eq!(read_page(503, "").unwrap_err().field, "status");
    assert_eq!(page_query("category=linear", Some("abc%3D")), "category=linear&cursor=abc%3D");
}

fn gathered() -> Gathered {
    Gathered { orders: rows(OPEN_ORDERS), executions: rows(EXECUTIONS), positions: Vec::new() }
}

/// Queried by settle coin, the venue lists only open positions, so a catalogue symbol it did not
/// list is an explicit flat.
#[test]
fn a_catalogue_symbol_the_venue_did_not_list_is_an_explicit_flat() {
    let catalogue = [bijective_model::VenueSymbol::new("ETHUSDT")];
    let (evidence, refused) = assemble(&gathered(), &catalogue, &scales(), 5, 9);
    assert!(refused.is_empty(), "{refused:?}");
    assert!(evidence.complete);
    let bijective_lifecycle::PositionEvidence::Reported(positions) = &evidence.positions else {
        panic!()
    };
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].quantity_scaled, 0);
    assert_eq!((evidence.orders.len(), evidence.fills.len()), (1, 1));
    assert_eq!(evidence.history_from_ns, Some(5));
}

/// One unreadable row makes the whole read incomplete, and names itself.
#[test]
fn a_refused_row_makes_the_evidence_incomplete() {
    let mut g = gathered();
    g.executions[0]["execQty"] = Value::from("not a number");
    let (evidence, refused) = assemble(&g, &[], &scales(), 5, 9);
    assert!(!evidence.complete);
    assert_eq!(refused[0].field, "execQty");
    assert!(evidence.fills.is_empty());
}

#[test]
fn an_order_listed_open_and_closed_keeps_its_latest_state() {
    let mut g = gathered();
    let mut closed = g.orders[0].clone();
    closed["orderStatus"] = Value::from("Cancelled");
    closed["updatedTime"] = Value::from("1684738549999");
    g.orders.push(closed);
    let (evidence, _) = assemble(&g, &[], &scales(), 5, 9);
    assert_eq!(evidence.orders.len(), 1);
    assert_eq!(evidence.orders[0].status, ReportedStatus::Canceled);
}

/// Execution history is read in windows of at most seven days, and an unbounded range is refused.
#[test]
fn history_is_read_in_seven_day_windows() {
    use crate::rest::windows;
    let day: i128 = 86_400_000_000_000;
    let w = windows(0, 10 * day).unwrap();
    assert_eq!(w.len(), 2);
    assert_eq!(w[0], (0, 7 * 86_400_000));
    assert_eq!(w[1].1, 10 * 86_400_000);
    assert!(windows(0, 200 * day).is_err());
    assert!(windows(5 * day, day).is_err());
}

/// A closed order from the recent list that closed before the window is outside it. One inside
/// it, and one with no readable time, are kept.
#[test]
fn a_closed_order_counts_only_inside_the_window() {
    let at = |t: &str| serde_json::json!({ "updatedTime": t });
    assert!(!super::changed_since(&at("999"), 1_000));
    assert!(super::changed_since(&at("1000"), 1_000));
    assert!(super::changed_since(&serde_json::json!({}), 1_000));
    assert!(super::changed_since(&at("soon"), 1_000));
}

fn pushed_fill() -> FillReport {
    let frame: Value = serde_json::from_str(STREAM_EXECUTION).unwrap();
    match execution(&frame["data"][0], &scales(), 11).unwrap() {
        Execution::Fill(report) => *report,
        Execution::Funding(_) => panic!("not a fill"),
    }
}

/// A fill the venue pushed before its history lists it joins the evidence once, and one executed
/// before the window is left out, as the history would leave it.
#[test]
fn a_pushed_fill_joins_the_evidence_the_history_has_not_listed_it_in() {
    let pushed = pushed_fill();
    let executed_at: i128 = 1_746_270_400_353 * 1_000_000;
    let (mut evidence, _) = assemble(&Gathered::default(), &[], &scales(), executed_at, 9);
    with_pushed(&mut evidence, std::slice::from_ref(&pushed), executed_at);
    assert_eq!(evidence.fills, vec![pushed.clone()]);
    with_pushed(&mut evidence, std::slice::from_ref(&pushed), executed_at);
    assert_eq!(evidence.fills.len(), 1, "a fill the read holds is not added twice");

    let (mut later, _) = assemble(&Gathered::default(), &[], &scales(), executed_at + 1, 9);
    with_pushed(&mut later, &[pushed], executed_at + 1);
    assert!(later.fills.is_empty());
}

/// Two instruments of one venue read at their own scales. A symbol the profile does not list is
/// read at the outside-catalogue scale, so its presence is judged and its digits never refused.
#[test]
fn each_instrument_reads_at_its_own_scales_and_an_unlisted_one_at_the_wide_scale() {
    let pair = |quantity| Scales {
        price: bijective_venue::normalise::Scale::new(2).unwrap(),
        quantity: bijective_venue::normalise::Scale::new(quantity).unwrap(),
    };
    let scales: InstrumentScales =
        [(VenueSymbol::new("BTCUSDT"), pair(3)), (VenueSymbol::new("ETHUSDT"), pair(2))]
            .into_iter()
            .collect();
    let position = |symbol: &str, size: &str| serde_json::json!({ "positionIdx": 0, "symbol": symbol, "side": "Buy", "size": size });
    let read = |symbol, size| position_report(&position(symbol, size), &scales).unwrap();
    assert_eq!(read("BTCUSDT", "0.001").quantity_scaled, 1);
    assert_eq!(read("ETHUSDT", "0.01").quantity_scaled, 1);
    assert_eq!(
        position_report(&position("ETHUSDT", "0.001"), &scales).unwrap_err().field,
        "size",
        "finer than the instrument's own scale is refused, not cut"
    );
    assert_eq!(read("SOLUSDT", "0.1").quantity_scaled, 100_000_000_000);
}

/// A call that got no answer says why, so an operator reads the cause and not a bare status.
#[test]
fn a_call_with_no_answer_carries_its_cause() {
    let refused = read_page(0, "operation timed out").unwrap_err();
    assert_eq!(refused.field, "status");
    assert!(refused.detail.contains("operation timed out"), "{}", refused.detail);
}

/// The unified account's wallet-balance row as read on 2026-11-19, coins left out.
const WALLET_ROW: &str = r#"{"accountType": "UNIFIED", "totalEquity": "164711.71572322", "totalMarginBalance": "99941.30602922", "totalWalletBalance": "99941.80752822", "totalPerpUPL": "-0.501499", "totalMaintenanceMargin": "0.34156975", "totalInitialMargin": "8.92614259", "accountMMRate": "0"}"#;

/// Margin balance, not total equity: a coin the account has not switched on as collateral
/// backs nothing, and `totalEquity` counts it.
#[test]
fn the_wallet_row_gives_the_margin_balance_and_the_maintenance_margin() {
    let row: Value = serde_json::from_str(WALLET_ROW).expect("json");
    let margin = account_margin(&row).expect("read").expect("stated");
    assert_eq!(margin.currency, "usd");
    assert_eq!(margin.scale, FEE_SCALE);
    assert_eq!(margin.equity_scaled, 99_941_306_029_220_000);
    assert_eq!(margin.maintenance_scaled, 341_569_750_000);
    let mut isolated = row.clone();
    isolated["accountType"] = Value::from("CONTRACT");
    assert_eq!(account_margin(&isolated).unwrap_err().field, "accountType");
    let mut isolated_mode = row.clone();
    isolated_mode["totalMarginBalance"] = Value::from("");
    isolated_mode["totalMaintenanceMargin"] = Value::from("");
    assert_eq!(account_margin(&isolated_mode), Ok(None), "an isolated-margin account states none");
}
