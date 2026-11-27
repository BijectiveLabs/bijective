// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Orders built from platform requests, and the venue's documented answers read into outcomes.

use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_venue::normalise::{Rejection, Unknown, VenueOutcome};

use super::*;
use crate::signing::{Exchange, Signer};

const TOKEN: &str = "32338220190071351435772801779725302244575775216413325951443816017994629993401";

fn signer() -> Signer {
    let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    let bytes: Vec<u8> =
        (0..32).map(|i| u8::from_str_radix(&key[2 * i..2 * i + 2], 16).expect("hex")).collect();
    Signer::from_secret(&bytes).expect("key")
}

fn maker() -> Maker {
    Maker { address: signer().address(), signature_type: 0, exchange: Exchange::Ctf }
}

/// Ten shares at 0.5000: scale 4 price, scale 6 quantity.
fn request(side: Side) -> OrderRequest {
    OrderRequest {
        client_id: "bl-1".into(),
        instrument: VenueSymbol::new(TOKEN),
        side,
        quantity_scaled: 10_000_000,
        price_scaled: Some(5_000),
        reduce_only: false,
        terms: Default::default(),
    }
}

/// A buy gives collateral and takes shares. A sell the reverse (Place orders).
#[test]
fn a_buy_gives_collateral_and_a_sell_gives_shares() {
    let buy = unsigned(&request(Side::Buy), &maker(), 1_780_449_126_930_000_000).expect("buy");
    assert_eq!((buy.maker_amount, buy.taker_amount, buy.side), (5_000_000, 10_000_000, 0));
    assert_eq!(buy.timestamp_ms, 1_780_449_126_930);
    let sell = unsigned(&request(Side::Sell), &maker(), 0).expect("sell");
    assert_eq!((sell.maker_amount, sell.taker_amount, sell.side), (10_000_000, 5_000_000, 1));
}

/// One client identifier, one salt, within the range a JSON number keeps exactly.
#[test]
fn the_salt_is_fixed_by_the_client_identifier() {
    assert_eq!(salt_for("bl-1"), salt_for("bl-1"));
    assert_ne!(salt_for("bl-1"), salt_for("bl-2"));
    assert!(salt_for("bl-1") < 1 << 53);
}

#[test]
fn a_request_the_venue_cannot_express_is_refused_before_signing() {
    let market = OrderRequest { price_scaled: None, ..request(Side::Buy) };
    assert_eq!(unsigned(&market, &maker(), 0), Err(OrderError::NoPrice));
    let named = OrderRequest { instrument: VenueSymbol::new("BTC"), ..request(Side::Buy) };
    assert_eq!(unsigned(&named, &maker(), 0), Err(OrderError::NotAnAsset));
    let fine = OrderRequest { quantity_scaled: 1, price_scaled: Some(5_001), ..request(Side::Buy) };
    assert_eq!(unsigned(&fine, &maker(), 0), Err(OrderError::NotRepresentable));
}

/// The body names the order hash it carries and never sets the builder field.
#[test]
fn the_body_carries_the_signed_order_and_no_attribution() {
    let signed =
        sign(&request(Side::Buy), &maker(), &signer(), "key-1", 1_000_000).expect("signed");
    let body: serde_json::Value = serde_json::from_str(&signed.body).expect("json");
    assert_eq!(body["owner"], "key-1");
    assert_eq!(body["orderType"], "GTC");
    assert_eq!(body["order"]["builder"], ZERO_WORD);
    assert_eq!(body["order"]["side"], "BUY");
    assert_eq!(body["order"]["tokenId"], TOKEN);
    assert!(body["order"]["salt"].is_u64());
    assert_eq!(signed.order_id.len(), 66);
    let again = sign(&request(Side::Buy), &maker(), &signer(), "key-1", 1_000_000).expect("signed");
    assert_eq!(again, signed, "one request at one instant signs one order");
}

/// The documented examples of `POST /order` and `DELETE /order` (CLOB API).
#[test]
fn the_venues_documented_answers_are_read() {
    let live = r#"{"success":true,"orderID":"0xabc","status":"live","makingAmount":"1","takingAmount":"2","errorMsg":""}"#;
    assert_eq!(read_submit(200, live), VenueOutcome::Accepted);
    assert_eq!(
        read_submit(400, r#"{"error":"Invalid order payload"}"#),
        VenueOutcome::Rejected(Rejection::Unclassified {
            venue_code: "Invalid order payload".into()
        })
    );
    assert_eq!(
        read_submit(401, r#"{"error":"Unauthorized/Invalid api key"}"#),
        VenueOutcome::Rejected(Rejection::CredentialRefused)
    );
    assert_eq!(read_submit(503, ""), VenueOutcome::Unknown(Unknown::ConnectionLost));
    assert_eq!(read_submit(200, "not json"), VenueOutcome::Unknown(Unknown::Unreadable));

    let canceled = r#"{"canceled":["0xabc"],"not_canceled":{}}"#;
    assert_eq!(read_cancel(200, canceled, "0xabc"), VenueOutcome::Accepted);
    let refused = r#"{"canceled":[],"not_canceled":{"0xabc":"order not found"}}"#;
    assert_eq!(
        read_cancel(200, refused, "0xabc"),
        VenueOutcome::Rejected(Rejection::Unclassified { venue_code: "order not found".into() })
    );
}

/// A protocol V2 book states its version. A CTF book omits it and states `neg_risk` (Place
/// orders). The CTF shape is the live answer read on 2026-11-16.
#[test]
fn a_book_names_the_exchange_its_orders_are_signed_for() {
    let ctf = r#"{"market":"0xa4","asset_id":"1","min_order_size":"5","tick_size":"0.001","neg_risk":false}"#;
    assert_eq!(exchange_of_book(ctf), Some(Exchange::Ctf));
    assert_eq!(exchange_of_book(r#"{"neg_risk":true}"#), Some(Exchange::CtfNegativeRisk));
    assert_eq!(exchange_of_book(r#"{"version":"v2","neg_risk":true}"#), Some(Exchange::ProtocolV2));
    assert_eq!(exchange_of_book(r#"{"version":"v3"}"#), None);
    assert_eq!(exchange_of_book(r#"{"tick_size":"0.01"}"#), None);
}

#[test]
fn a_cancel_answer_is_matched_without_case() {
    let canceled = r#"{"canceled":["0xABC"],"not_canceled":{}}"#;
    assert_eq!(read_cancel(200, canceled, "0xabc"), VenueOutcome::Accepted);
    let refused = r#"{"canceled":[],"not_canceled":{"0xABC":"already canceled"}}"#;
    assert_eq!(
        read_cancel(200, refused, "0xabc"),
        VenueOutcome::Rejected(Rejection::Unclassified { venue_code: "already canceled".into() })
    );
}

/// A Deposit Wallet order names the wallet as maker and signer. A Proxy or Safe kind is refused.
#[test]
fn a_deposit_wallet_order_is_the_wallets_and_other_kinds_are_refused() {
    let wallet = [0x2e; 20];
    let deposit =
        Maker { address: wallet, signature_type: DEPOSIT_WALLET, exchange: Exchange::Ctf };
    let signed = sign(&request(Side::Buy), &deposit, &signer(), "k", 1_000_000).expect("signed");
    let body: serde_json::Value = serde_json::from_str(&signed.body).expect("json");
    let named = format!("0x{}", hex(&wallet));
    assert_eq!(
        (&body["order"]["maker"], &body["order"]["signer"]),
        (&named.clone().into(), &named.into())
    );
    assert_eq!(body["order"]["signatureType"], 3);
    assert!(body["order"]["signature"].as_str().expect("text").len() > 2 + 2 * 65);

    let proxy = Maker { signature_type: 1, ..deposit };
    assert_eq!(
        sign(&request(Side::Buy), &proxy, &signer(), "k", 0),
        Err(OrderError::WalletKind(1))
    );
}

/// A restored order is recognised by the hash it signs to on its exchange, whichever it was.
#[test]
fn each_candidate_is_the_hash_the_order_signs_to_on_that_exchange() {
    let request = request(Side::Buy);
    let candidates = candidate_ids(&request, &maker(), 1_794_819_035_000_000_000).expect("hashed");

    for (exchange, candidate) in Exchange::ALL.into_iter().zip(&candidates) {
        let maker = Maker { exchange, ..maker() };
        let signed = sign(&request, &maker, &signer(), "key", 1_794_819_035_000_000_000);
        assert_eq!(&signed.expect("signed").order_id, candidate);
    }
    assert_ne!(candidates[0], candidates[1]);
    assert_ne!(candidates[1], candidates[2]);
}
