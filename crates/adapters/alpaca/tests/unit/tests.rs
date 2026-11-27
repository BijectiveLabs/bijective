// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Orders built from platform requests, the venue's documented answers read into outcomes, and
//! its documented rows read into evidence.

use std::collections::BTreeMap;

use bijective_lifecycle::ReportedStatus;
use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::{Rejection, Scale, Unknown, VenueOutcome};

use crate::evidence::{
    Scales, charge_report, fill_report, order_report, position_report, utc_ns, utc_text,
};
use crate::order::{OrderError, limit_price_text, order_body, read_cancel, read_submit};

fn scales() -> Scales {
    Scales { price: Scale::new(4).expect("price"), quantity: Scale::new(0).expect("quantity") }
}

/// Ten shares at 150.12.
fn request(price_scaled: Option<i128>, reduce_only: bool) -> OrderRequest {
    OrderRequest {
        client_id: "bl-1".into(),
        instrument: VenueSymbol::new("AAPL"),
        side: Side::Buy,
        quantity_scaled: 10,
        price_scaled,
        reduce_only,
        terms: Default::default(),
    }
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("JSON")
}

#[test]
fn a_limit_price_is_written_on_the_venue_grid_or_refused() {
    let scale = Scale::new(4).expect("scale");
    assert_eq!(limit_price_text(1_500_000, scale), Ok("150".into()));
    assert_eq!(limit_price_text(1_501_200, scale), Ok("150.12".into()));
    assert_eq!(limit_price_text(1_234, scale), Ok("0.1234".into()), "four places below a dollar");
    assert!(matches!(limit_price_text(1_501_230, scale), Err(OrderError::SubPenny { .. })));
    let fine = Scale::new(9).expect("scale");
    assert!(matches!(limit_price_text(123_450_000, fine), Err(OrderError::SubPenny { .. })));
}

/// A limit order rests until cancelled. Nothing but the documented fields is written.
#[test]
fn a_limit_order_is_good_till_cancelled_in_whole_shares() {
    let body = json(
        &order_body(&request(Some(1_501_200), false), scales().price, scales().quantity)
            .expect("built"),
    );

    assert_eq!(body["type"], "limit");
    assert_eq!(body["time_in_force"], "gtc");
    assert_eq!(body["limit_price"], "150.12");
    assert_eq!(body["qty"], "10");
    assert_eq!(body["side"], "buy");
    assert_eq!(body["client_order_id"], "bl-1");
    assert_eq!(body.as_object().expect("object").len(), 7, "{body}");
}

/// A market order only reduces: queued outside the session, an entry would execute at the next
/// open on a decision made before it.
#[test]
fn a_market_order_is_sent_only_to_reduce() {
    let (price, quantity) = (scales().price, scales().quantity);
    assert_eq!(order_body(&request(None, false), price, quantity), Err(OrderError::MarketEntry));
    let exit = json(&order_body(&request(None, true), price, quantity).expect("built"));
    assert_eq!(
        (exit["type"].as_str(), exit["time_in_force"].as_str()),
        (Some("market"), Some("day"))
    );
    let long = OrderRequest { client_id: "x".repeat(129), ..request(Some(1_501_200), false) };
    assert_eq!(order_body(&long, price, quantity), Err(OrderError::ClientIdTooLong));
}

#[test]
fn a_submit_is_accepted_only_when_the_order_returned_is_this_one() {
    let order = r#"{"id":"61e69015-8549-4bfd-b9c3-01e75843f47d","client_order_id":"bl-1","status":"accepted"}"#;
    assert_eq!(read_submit(200, order, "bl-1"), VenueOutcome::Accepted);
    assert_eq!(read_submit(200, order, "bl-2"), VenueOutcome::Unknown(Unknown::Unreadable));
    assert_eq!(read_submit(200, "not json", "bl-1"), VenueOutcome::Unknown(Unknown::Unreadable));
}

/// A refusal carries the venue's own words. A server error or no answer may have placed it.
#[test]
fn refusals_travel_with_the_venue_s_words_and_server_errors_are_unknown() {
    let sub_penny = r#"{"code": 42210000, "message": "invalid limit_price 290.123. sub-penny increment does not fulfill minimum pricing criteria"}"#;
    let VenueOutcome::Rejected(Rejection::Unclassified { venue_code }) =
        read_submit(422, sub_penny, "bl-1")
    else {
        panic!("a refusal");
    };
    assert!(venue_code.contains("42210000") && venue_code.contains("sub-penny"), "{venue_code}");
    assert!(matches!(read_submit(403, "{}", "bl-1"), VenueOutcome::Rejected(_)));
    let reused = r#"{"code":40010001,"message":"client_order_id must be unique"}"#;
    assert_eq!(read_submit(422, reused, "bl-1"), VenueOutcome::Rejected(Rejection::Duplicate));
    assert_eq!(read_submit(401, "", "bl-1"), VenueOutcome::Rejected(Rejection::CredentialRefused));
    assert_eq!(
        read_submit(429, "", "bl-1"),
        VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None })
    );
    assert_eq!(read_submit(503, "", "bl-1"), VenueOutcome::Unknown(Unknown::ConnectionLost));
    assert_eq!(read_submit(0, "", "bl-1"), VenueOutcome::Unknown(Unknown::ConnectionLost));
    assert_eq!(read_cancel(204, ""), VenueOutcome::Accepted);
    assert!(matches!(read_cancel(422, "{}"), VenueOutcome::Rejected(_)));
}

/// The venue's examples: microseconds with `Z`, nanoseconds, and an offset.
#[test]
fn venue_timestamps_are_read_as_utc_nanoseconds() {
    let at = 1_643_828_109_981_482_000;
    assert_eq!(utc_ns("2022-02-02T18:55:09.981482Z"), Some(at));
    assert_eq!(utc_ns("2022-02-02T13:55:09.981482-05:00"), Some(at));
    assert_eq!(
        utc_ns("2023-12-12T22:31:24.577215743Z").map(|n| n % 1_000_000_000),
        Some(577_215_743)
    );
    assert_eq!(utc_ns("2022-02-02T18:55:09Z"), Some(1_643_828_109_000_000_000));
    assert_eq!(utc_ns("2022-13-02T18:55:09Z"), None);
    assert_eq!(utc_ns("1643828109"), None);
    assert_eq!(utc_text(at), "2022-02-02T18:55:09Z");
    assert_eq!(utc_text(951_782_400_000_000_000), "2000-02-29T00:00:00Z");
}

const ORDER: &str = r#"{"id":"61e69015-8549-4bfd-b9c3-01e75843f47d","client_order_id":"bl-1","symbol":"AAPL","side":"buy","qty":"10","filled_qty":"4","status":"partially_filled","updated_at":"2022-02-02T18:55:09.981482Z"}"#;

#[test]
fn an_order_is_read_with_its_client_identifier_and_status() {
    let report = order_report(&json(ORDER), scales()).expect("read");
    assert_eq!(report.client_id.as_deref(), Some("bl-1"));
    assert_eq!((report.ordered_quantity_scaled, report.filled_quantity_scaled), (10, 4));
    assert_eq!(report.status, ReportedStatus::Working);
    assert_eq!(report.reported_at, ExchangeTime::At { at_ns: 1_643_828_109_981_482_000 });

    for (status, read) in [
        ("filled", ReportedStatus::Filled),
        ("canceled", ReportedStatus::Canceled),
        ("expired", ReportedStatus::Expired),
        ("rejected", ReportedStatus::Rejected),
        ("done_for_day", ReportedStatus::Working),
    ] {
        let row = ORDER.replace("partially_filled", status);
        assert_eq!(order_report(&json(&row), scales()).expect(status).status, read);
    }
    for status in ["held", "replaced", "calculated", "something_new"] {
        let row = ORDER.replace("partially_filled", status);
        assert!(order_report(&json(&row), scales()).is_err(), "{status} is refused");
    }
    let fractional = ORDER.replace(r#""qty":"10""#, r#""qty":"0.5""#);
    assert!(order_report(&json(&fractional), scales()).is_err(), "whole shares only");
}

/// A fill names only the venue's order. The order read links it to the client identifier.
#[test]
fn a_fill_is_linked_through_its_order_and_states_no_fee() {
    let row = json(
        r#"{"id":"20220202135509981::2d7be4ff-d1f3-43e9-856a-0f5cf5c5088e","activity_type":"FILL","transaction_time":"2022-02-02T18:55:09.981482Z","type":"partial_fill","price":"150.12","qty":"4","side":"buy","symbol":"AAPL","leaves_qty":"6","order_id":"61e69015-8549-4bfd-b9c3-01e75843f47d","cum_qty":"4","order_status":"partially_filled"}"#,
    );
    let mut clients = BTreeMap::new();
    let external = fill_report(&row, &clients, scales(), 7).expect("read");
    assert_eq!(external.client_id, None, "an order not read is not claimed");

    clients.insert("61e69015-8549-4bfd-b9c3-01e75843f47d".to_string(), "bl-1".to_string());
    let fill = fill_report(&row, &clients, scales(), 7).expect("read");
    assert_eq!(fill.client_id.as_deref(), Some("bl-1"));
    assert_eq!(fill.fill.fill_id, "20220202135509981::2d7be4ff-d1f3-43e9-856a-0f5cf5c5088e");
    assert_eq!((fill.fill.price_scaled, fill.fill.quantity_scaled), (1_501_200, 4));
    assert_eq!(fill.fill.fee, None);
}

/// The side decides the sign, whether or not a short's quantity carries one.
#[test]
fn a_short_position_is_negative_whatever_its_quantity_says() {
    let long = r#"{"symbol":"AAPL","qty":"5","side":"long","avg_entry_price":"150.12"}"#;
    assert_eq!(position_report(&json(long), scales()).expect("long").quantity_scaled, 5);
    for qty in ["5", "-5"] {
        let short =
            long.replace(r#""qty":"5""#, &format!(r#""qty":"{qty}""#)).replace("long", "short");
        assert_eq!(position_report(&json(&short), scales()).expect("short").quantity_scaled, -5);
    }
}

/// A fee paid is negative in the account's activities and positive as a charge. A figure finer
/// than eight decimals is refused, and a missing currency is left empty, never assumed.
#[test]
fn a_fee_activity_is_a_charge_the_account_paid() {
    let paid = json(r#"{"id":"a::1","activity_type":"FEE","net_amount":"-0.02","currency":"USD"}"#);
    let charge = charge_report(&paid).expect("read");
    assert_eq!(charge.charge_id, "a::1");
    assert_eq!((charge.fee.asset.as_str(), charge.fee.amount_scaled), ("USD", 2_000_000));

    let unstated = json(r#"{"id":"a::2","net_amount":"-0.02"}"#);
    assert_eq!(charge_report(&unstated).expect("read").fee.asset, "");

    let fine = json(r#"{"id":"a::3","net_amount":"-0.000000001","currency":"USD"}"#);
    assert!(charge_report(&fine).is_err());
}

/// The paper account as read on 2026-11-20, the fields the margin reads.
#[test]
fn the_account_gives_its_equity_and_maintenance_margin_in_its_currency() {
    use crate::evidence::account_margin;
    let account: serde_json::Value = serde_json::from_str(
        r#"{"currency": "USD", "equity": "99999.38", "maintenance_margin": "0", "multiplier": "2"}"#,
    )
    .expect("json");
    let margin = account_margin(&account).expect("read");
    assert_eq!((margin.currency.as_str(), margin.scale), ("usd", 8));
    assert_eq!((margin.equity_scaled, margin.maintenance_scaled), (9_999_938_000_000, 0));
    let mut unnamed = account.clone();
    unnamed["currency"] = serde_json::Value::from("");
    assert_eq!(account_margin(&unnamed).unwrap_err().field, "currency");
}

/// A stop-limit enters. A stop-market only reduces. Neither rests past the day or good till
/// cancelled (decision 0012).
#[test]
fn a_stop_is_a_stop_or_stop_limit_order() {
    let (price, quantity) = (scales().price, scales().quantity);
    let stop = |limit: Option<i128>, reduce_only: bool, time_in_force| {
        let mut o = request(limit, reduce_only);
        o.terms = bijective_model::OrderTerms {
            time_in_force,
            post_only: false,
            stop_price_scaled: Some(1_500_000),
            expire_at_ns: None,
        };
        order_body(&o, price, quantity)
    };
    use bijective_model::TimeInForce as T;
    let entry = json(&stop(Some(1_501_200), false, T::GoodTillCancelled).expect("built"));
    assert_eq!(
        (entry["type"].as_str(), entry["time_in_force"].as_str()),
        (Some("stop_limit"), Some("gtc"))
    );
    assert!(entry["stop_price"].is_string() && entry["limit_price"].is_string());
    let exit = json(&stop(None, true, T::Day).expect("built"));
    assert_eq!(
        (exit["type"].as_str(), exit["time_in_force"].as_str()),
        (Some("stop"), Some("day"))
    );
    assert_eq!(stop(None, false, T::GoodTillCancelled), Err(OrderError::MarketEntry));
    assert!(matches!(
        stop(Some(1_501_200), false, T::ImmediateOrCancel),
        Err(OrderError::TermsNotTaken(_))
    ));
}
