// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The venue's documented order, trade and position rows read into evidence.

use bijective_lifecycle::{Liquidity, ReportedStatus};
use bijective_model::Side;
use bijective_model::market::ExchangeTime;
use bijective_venue::normalise::Scale;
use serde_json::json;

use super::*;

const HASH: &str = "0xff354cd7ca7539dfa9c28d90943ab5779a4eac34b9b37a757d7b32bdfb11790b";
const WALLET: &str = "0x1234567890123456789012345678901234567890";

fn scales() -> Scales {
    Scales { price: Scale::new(4).expect("price"), quantity: Scale::new(6).expect("quantity") }
}

fn clients() -> ClientOf {
    ClientOf::from([(HASH.to_string(), "bl-1".to_string())])
}

#[test]
fn a_page_is_read_as_the_reference_object_or_the_guides_bare_array() {
    let object = read_page(r#"{"limit":100,"next_cursor":"MTAw","count":1,"data":[{}]}"#);
    let last = read_page(r#"{"limit":100,"next_cursor":"LTE=","count":0,"data":[]}"#);
    let array = read_page("[{},{}]");

    assert_eq!(object.expect("object").next_cursor.as_deref(), Some("MTAw"));
    assert_eq!(last.expect("last").next_cursor, None);
    assert_eq!(array.expect("array").rows.len(), 2);
    assert!(read_page(r#"{"next_cursor":"LTE="}"#).is_err(), "no data is not an empty page");
}

/// The reference's `OpenOrder` example: sizes in decimal shares, already normalised.
#[test]
fn an_order_row_names_its_client_through_the_hash_this_client_signed() {
    let row = json!({
        "id": HASH.to_ascii_uppercase().replacen("0X", "0x", 1), "status": "LIVE",
        "asset_id": "107505882767731489358349912513945399560393482969656700824895970500493757150417",
        "side": "BUY", "original_size": "10", "size_matched": "2.5", "price": "0.52",
        "created_at": 1_700_000_000
    });

    let report = order_report(&row, &clients(), scales()).expect("read");

    assert_eq!(report.client_id.as_deref(), Some("bl-1"));
    assert_eq!(report.venue_order_id, HASH);
    assert_eq!(
        (report.ordered_quantity_scaled, report.filled_quantity_scaled),
        (10_000_000, 2_500_000)
    );
    assert_eq!(report.status, ReportedStatus::Working);
    assert_eq!(report.reported_at, ExchangeTime::At { at_ns: 1_700_000_000_000_000_000 });
}

#[test]
fn an_order_this_client_did_not_sign_is_external_and_an_unknown_status_is_refused() {
    let row = json!({
        "id": "0x01", "status": "MATCHED", "asset_id": "1", "side": "SELL",
        "original_size": "1", "size_matched": "1"
    });
    let report = order_report(&row, &clients(), scales()).expect("read");
    assert_eq!((report.client_id, report.status), (None, ReportedStatus::Filled));

    let odd = json!({
        "id": "0x01", "status": "DELAYED", "asset_id": "1", "side": "SELL",
        "original_size": "1", "size_matched": "0"
    });
    assert_eq!(order_report(&odd, &clients(), scales()).expect_err("refused").field, "status");
}

#[test]
fn a_taker_trade_is_one_fill_of_this_wallets_order() {
    let row = json!({
        "id": "t-1", "taker_order_id": HASH, "asset_id": "7", "side": "BUY", "size": "10",
        "price": "0.52", "status": "TRADE_STATUS_CONFIRMED", "match_time": "1748779205",
        "match_time_nano": "1748779205123456789", "trader_side": "TAKER", "maker_orders": []
    });

    let reading = trade_reading(&row, WALLET, &clients(), scales(), 9).expect("read");

    assert!(reading.settled);
    let fill = &reading.fills[0];
    assert_eq!(fill.client_id.as_deref(), Some("bl-1"));
    assert_eq!(fill.fill.fill_id, "t-1");
    assert_eq!((fill.fill.price_scaled, fill.fill.quantity_scaled), (5_200, 10_000_000));
    assert_eq!(fill.fill.liquidity, Liquidity::Taker);
    assert_eq!(fill.fill.exchange_time, ExchangeTime::At { at_ns: 1_748_779_205_123_456_789 });
}

/// The maker's own entries carry the fill: a maker can rest on the complementary outcome, so its
/// asset, side and price are its own and not the taker's.
#[test]
fn a_maker_trade_is_read_from_this_wallets_maker_entries_only() {
    let row = json!({
        "id": "t-2", "taker_order_id": "0xother", "asset_id": "7", "side": "BUY", "size": "10",
        "price": "0.52", "status": "MATCHED", "match_time": "1748779205", "trader_side": "MAKER",
        "maker_orders": [
            { "order_id": HASH, "maker_address": WALLET.to_ascii_uppercase().replacen("0X", "0x", 1),
              "asset_id": "8", "side": "BUY", "price": "0.48", "matched_amount": "6" },
            { "order_id": "0xsomeone", "maker_address": "0x9999999999999999999999999999999999999999",
              "asset_id": "7", "side": "SELL", "price": "0.52", "matched_amount": "4" }
        ]
    });

    let reading = trade_reading(&row, WALLET, &clients(), scales(), 9).expect("read");

    assert!(!reading.settled, "a matched trade can still fail on chain");
    assert_eq!(reading.fills.len(), 1);
    let fill = &reading.fills[0];
    assert_eq!(fill.instrument.as_str(), "8");
    assert_eq!(fill.fill.side, Side::Buy);
    assert_eq!((fill.fill.price_scaled, fill.fill.quantity_scaled), (4_800, 6_000_000));
    assert_eq!(fill.fill.fill_id, format!("t-2:{HASH}"));
    assert_eq!(fill.fill.liquidity, Liquidity::Maker);
}

#[test]
fn a_failed_trade_and_a_maker_trade_without_this_wallet_are_refused() {
    let failed = json!({ "id": "t-3", "status": "FAILED", "trader_side": "TAKER" });
    let foreign = json!({
        "id": "t-4", "status": "CONFIRMED", "trader_side": "MAKER",
        "maker_orders": [{ "maker_address": "0x9999999999999999999999999999999999999999" }]
    });

    assert_eq!(
        trade_reading(&failed, WALLET, &clients(), scales(), 0).expect_err("x").field,
        "status"
    );
    assert_eq!(
        trade_reading(&foreign, WALLET, &clients(), scales(), 0).expect_err("x").field,
        "maker_orders"
    );
}

/// The data service's documented page (Wallet activity): sizes are doubles in shares.
#[test]
fn a_position_page_keeps_shares_exact_and_rounds_the_average_price() {
    let body = json!({
        "data": [{ "token_id": "7", "current_size": 125.5, "avg_price": 0.421_37 }],
        "pagination": { "limit": 1, "offset": 0, "has_more": true, "next_cursor": "eyJ9" }
    })
    .to_string();

    let page = position_page(&body, scales()).expect("read");

    assert_eq!(page.positions[0].quantity_scaled, 125_500_000);
    assert_eq!(page.positions[0].entry_price_scaled, 4_214);
    assert_eq!(page.next_cursor.as_deref(), Some("eyJ9"));
}

#[test]
fn a_position_page_that_cannot_be_read_whole_or_exactly_is_refused() {
    let finer = json!({
        "data": [{ "token_id": "7", "current_size": 0.000_000_1, "avg_price": 0.5 }],
        "pagination": { "has_more": false }
    });
    let no_cursor = json!({ "data": [], "pagination": { "has_more": true } });
    let no_pagination = json!({ "data": [] });

    for body in [finer, no_cursor, no_pagination] {
        assert!(position_page(&body.to_string(), scales()).is_err(), "{body}");
    }
}

/// Decision 0002, rule 6: the venue's matched size counts an unconfirmed trade. It is taken out.
#[test]
fn an_unconfirmed_trade_is_held_out_of_its_orders_filled_size() {
    let row = json!({
        "id": HASH, "status": "MATCHED", "asset_id": "7", "side": "BUY",
        "original_size": "10", "size_matched": "10"
    });
    let mut orders = vec![order_report(&row, &clients(), scales()).expect("read")];

    hold_unconfirmed(&mut orders, &BTreeMap::from([(HASH.to_string(), 4_000_000)]));

    assert_eq!(orders[0].filled_quantity_scaled, 6_000_000);
    assert_eq!(orders[0].status, ReportedStatus::Working);
}
