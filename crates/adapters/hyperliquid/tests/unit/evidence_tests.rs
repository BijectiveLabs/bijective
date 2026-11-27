// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Rows shaped as the venue's live answers (read 2026-11-23), judged into evidence.

use bijective_lifecycle::{Liquidity, ReportedStatus};
use bijective_model::market::ExchangeTime;
use bijective_venue::normalise::Scale;
use serde_json::{Value, json};

use super::*;

fn quantity() -> Scale {
    Scale::new(4).expect("scale")
}

fn price() -> Scale {
    Scale::new(1).expect("scale")
}

fn ours() -> Clients {
    let mut clients = Clients::default();
    clients.placed("bl-1");
    clients
}

fn entry(cloid: &str, status: &str, sz: &str, at_ms: u64) -> Value {
    json!({
        "order": {
            "coin": "ETH", "side": "A", "limitPx": "1719.9", "sz": sz, "oid": 613_808_845_382_u64,
            "timestamp": 1_795_422_557_372_u64, "triggerCondition": "N/A", "isTrigger": false,
            "triggerPx": "0.0", "children": [], "isPositionTpsl": false, "reduceOnly": false,
            "orderType": "Limit", "origSz": "0.8923", "tif": "Alo", "cloid": cloid
        },
        "status": status,
        "statusTimestamp": at_ms
    })
}

#[test]
fn an_order_of_ours_is_named_and_its_filled_part_is_what_no_longer_remains() {
    let cloid = cloid_for("bl-1").to_ascii_uppercase().replace("0X", "0x");
    let report =
        order_report(&entry(&cloid, "canceled", "0.0364", 5), &ours(), quantity()).expect("judged");
    assert_eq!(report.client_id.as_deref(), Some("bl-1"));
    assert_eq!(report.venue_order_id, "613808845382");
    assert_eq!(report.side, Side::Sell);
    assert_eq!((report.ordered_quantity_scaled, report.filled_quantity_scaled), (8923, 8559));
    assert_eq!(report.status, ReportedStatus::Canceled);
    assert_eq!(report.reported_at, ExchangeTime::from_ms(5));
}

#[test]
fn an_order_not_placed_here_is_external() {
    let foreign = "0x0000000000000000000001a1f7197daa";
    let report =
        order_report(&entry(foreign, "open", "0.8923", 5), &ours(), quantity()).expect("judged");
    assert_eq!(report.client_id, None);
    assert_eq!(report.status, ReportedStatus::Working);
}

#[test]
fn every_documented_finish_maps_and_anything_else_is_refused() {
    for (raw, expected) in [
        ("filled", ReportedStatus::Filled),
        ("scheduledCancel", ReportedStatus::Canceled),
        ("siblingFilledCanceled", ReportedStatus::Canceled),
        ("badAloPxRejected", ReportedStatus::Rejected),
        ("perpMaxPositionRejected", ReportedStatus::Rejected),
    ] {
        assert_eq!(status(raw), Ok(expected), "{raw}");
    }
    for raw in ["triggered", "somethingNew", ""] {
        assert!(status(raw).is_err(), "{raw}");
    }
}

/// The history lists the newest change first. The finished state is kept whatever the order.
#[test]
fn the_latest_change_of_an_order_is_kept() {
    let cloid = cloid_for("bl-1");
    let judged =
        |status, at| order_report(&entry(&cloid, status, "0.8923", at), &ours(), quantity());
    for listed in [[("canceled", 20), ("open", 10)], [("open", 10), ("canceled", 20)]] {
        let mut orders = Vec::new();
        for (status, at) in listed {
            keep_latest(&mut orders, judged(status, at).expect("judged"));
        }
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].status, ReportedStatus::Canceled);
    }
    let mut same_stamp = Vec::new();
    keep_latest(&mut same_stamp, judged("canceled", 20).expect("judged"));
    keep_latest(&mut same_stamp, judged("open", 20).expect("judged"));
    assert_eq!(same_stamp[0].status, ReportedStatus::Canceled, "finished outranks open");
}

#[test]
fn a_fill_carries_its_cloid_its_rebate_and_which_side_took_liquidity() {
    let row = json!({
        "coin": "ETH", "px": "1682.7", "sz": "0.0849", "side": "A", "time": 1_795_336_159_864_u64,
        "startPosition": "2.5376", "dir": "Close Long", "closedPnl": "0.028866",
        "hash": "0x74be", "oid": 613_035_220_130_u64, "crossed": false, "fee": "-0.004285",
        "tid": 710_272_505_999_778_u64, "cloid": cloid_for("bl-1"), "feeToken": "USDC",
        "twapId": null
    });
    let report = fill_report(&row, &ours(), price(), quantity(), 7).expect("judged");
    assert_eq!(report.client_id.as_deref(), Some("bl-1"));
    assert_eq!(report.fill.fill_id, "710272505999778");
    assert_eq!(report.fill.venue_order_id.as_deref(), Some("613035220130"));
    assert_eq!((report.fill.price_scaled, report.fill.quantity_scaled), (16827, 849));
    assert_eq!(report.fill.liquidity, Liquidity::Maker);
    let fee = report.fill.fee.expect("a fee");
    assert_eq!((fee.asset.as_str(), fee.amount_scaled), ("USDC", -4_285_000_000));
}

/// Read from live answers: a long position at a positive rate shows a negative `usdc`, so the
/// charge, positive when paid, is its negation. The documentation's own example has a leading
/// space in the amount.
#[test]
fn funding_paid_is_a_positive_charge_named_by_coin_and_stamp() {
    let row = json!({
        "time": 1_795_165_200_028_u64,
        "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "delta": { "type": "funding", "coin": "ETH", "usdc": " -0.061987", "szi": "2.8316",
                   "fundingRate": "0.0000125", "nSamples": null }
    });
    let charge = funding_charge(&row).expect("judged");
    assert_eq!(charge.charge_id, "funding:ETH:1795165200028");
    assert_eq!((charge.fee.asset.as_str(), charge.fee.amount_scaled), ("USDC", 61_987_000_000));
}

#[test]
fn an_order_is_asked_for_until_the_venue_reports_it_finished() {
    let mut clients = ours();
    clients.adopt(&[]);
    assert_eq!(clients.unsettled(), [cloid_for("bl-1")]);
    let working =
        order_report(&entry(&cloid_for("bl-1"), "open", "0.8923", 5), &clients, quantity())
            .expect("judged");
    clients.settle(&[working]);
    assert_eq!(clients.unsettled().len(), 1, "still working");
    let filled = order_report(&entry(&cloid_for("bl-1"), "filled", "0.0", 6), &clients, quantity())
        .expect("judged");
    clients.settle(&[filled]);
    assert!(clients.unsettled().is_empty());
}
