// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Operator behaviour settled before any network call.

use super::*;
use bijective_common::clock::SequenceClock;

fn client(endpoint: &str, at_ms: i128) -> RestClient {
    let clock = SequenceClock::starting_at(at_ms * 1_000_000);
    RestClient::new(ClientSetup {
        endpoint,
        wallet_key: &Credential::new(vec![7u8; 32]),
        account_address: "0x00000000000000000000000000000000000000aa",
        price: Scale::new(2).expect("a scale"),
        quantity: Scale::new(3).expect("a scale"),
        timeout: Duration::from_secs(1),
        clock: Arc::new(clock),
        mainnet: true,
    })
    .expect("a client with a usable key")
}

fn order(client_id: &str, quantity: i128, price: Option<i128>) -> OrderRequest {
    OrderRequest {
        client_id: client_id.to_string(),
        instrument: VenueSymbol::new("BTC"),
        side: Side::Buy,
        quantity_scaled: quantity,
        price_scaled: price,
        reduce_only: false,
        terms: Default::default(),
    }
}

/// A wrong environment guess surfaces as a missing account.
#[test]
fn an_endpoint_names_its_environment_or_is_refused() {
    assert_eq!(environment_of("https://api.hyperliquid.xyz"), Some(true));
    assert_eq!(environment_of("https://api.hyperliquid-testnet.xyz"), Some(false));
    assert_eq!(environment_of("https://api.hyperliquid.xyz/TESTNET"), Some(false));
    assert_eq!(environment_of("https://example.com"), None);
    assert_eq!(environment_of(""), None);
}

/// A clock-only counter would repeat within one millisecond.
#[test]
fn two_actions_in_one_millisecond_do_not_share_a_nonce() {
    let mut client = client("https://api.hyperliquid.xyz", 1_700_000_000_000);
    let first = client.next_nonce();
    let second = client.next_nonce();
    let third = client.next_nonce();
    assert!(second > first, "{second} did not advance past {first}");
    assert!(third > second, "{third} did not advance past {second}");
}

#[test]
fn a_nonce_starts_from_the_clock_rather_than_from_zero() {
    let mut client = client("https://api.hyperliquid.xyz", 1_700_000_000_000);
    assert_eq!(client.next_nonce(), 1_700_000_000_000);
}

#[test]
fn a_key_the_curve_refuses_stops_the_client_being_built() {
    let clock = SequenceClock::starting_at(0);
    let refused = RestClient::new(ClientSetup {
        endpoint: "https://api.hyperliquid.xyz",
        wallet_key: &Credential::new(vec![0u8; 32]),
        account_address: "0x00000000000000000000000000000000000000aa",
        price: Scale::new(2).expect("a scale"),
        quantity: Scale::new(3).expect("a scale"),
        timeout: Duration::from_secs(1),
        clock: Arc::new(clock),
        mainnet: true,
    });
    assert!(matches!(refused, Err(ConnectError::NotPermitted { .. })));
}

#[tokio::test]
async fn an_order_with_no_identifier_is_refused_without_reaching_the_venue() {
    let mut client = client("https://api.hyperliquid.xyz", 1);
    let refused = client.submit(&order("", 1, Some(1))).await;
    assert_eq!(refused, Err(RequestError::EmptyClientId));
}

#[tokio::test]
async fn a_non_positive_quantity_is_refused_without_reaching_the_venue() {
    let mut client = client("https://api.hyperliquid.xyz", 1);
    let refused = client.submit(&order("0x01", 0, Some(1))).await;
    assert_eq!(refused, Err(RequestError::NonPositiveQuantity));
}

/// No market order. Not translated into an IOC limit at a price the strategy did not set.
#[tokio::test]
async fn an_order_with_no_price_is_refused_rather_than_translated() {
    let mut client = client("https://api.hyperliquid.xyz", 1);
    let refused = client.submit(&order("0x01", 1, None)).await;
    assert_eq!(refused, Err(RequestError::NoPrice));
}

#[tokio::test]
async fn a_cancel_with_no_identifier_is_refused_without_reaching_the_venue() {
    let mut client = client("https://api.hyperliquid.xyz", 1);
    let refused = client.cancel(&VenueSymbol::new("BTC"), "").await;
    assert_eq!(refused, Err(RequestError::EmptyClientId));
}

/// The venue's own example body, from its perpetuals info page.
const ACCOUNT_READ: &str = r#"{"assetPositions":[{"position":{"coin":"ETH","entryPx":"2986.3",
    "szi":"0.0335","unrealizedPnl":"-0.0134"},"type":"oneWay"}],"withdrawable":"1.0"}"#;

fn scales() -> (Scale, Scale) {
    (Scale::new(2).expect("a scale"), Scale::new(4).expect("a scale"))
}

#[test]
fn the_venues_own_example_reads_as_one_position() {
    let (price, quantity) = scales();
    let held = read_positions(ACCOUNT_READ, price, quantity).expect("the example was refused");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].instrument.as_str(), "ETH");
    assert_eq!(held[0].quantity_scaled, 335);
    assert_eq!(held[0].entry_price_scaled, 298_630);
}

#[test]
fn a_short_keeps_its_sign() {
    let (price, quantity) = scales();
    let body = r#"{"assetPositions":[{"position":{"coin":"BTC","entryPx":"100","szi":"-1.5"}}]}"#;
    let held = read_positions(body, price, quantity).expect("a short was refused");
    assert_eq!(held[0].quantity_scaled, -15_000);
}

/// An absent list is not an empty one. A false flat account makes reconciliation see every
/// position missing.
#[test]
fn a_body_with_no_position_list_is_refused_rather_than_read_as_flat() {
    let (price, quantity) = scales();
    let refused = read_positions(r#"{"withdrawable":"1.0"}"#, price, quantity);
    assert!(matches!(refused, Err(ConnectError::Transport { .. })));
}

#[test]
fn an_account_holding_nothing_is_not_the_same_as_one_we_could_not_read() {
    let (price, quantity) = scales();
    let held = read_positions(r#"{"assetPositions":[]}"#, price, quantity).expect("empty is fine");
    assert!(held.is_empty());
}

/// Valuing at zero would price a real position at nothing.
#[test]
fn a_holding_with_no_entry_price_is_refused() {
    let (price, quantity) = scales();
    let body = r#"{"assetPositions":[{"position":{"coin":"ETH","szi":"1.0"}}]}"#;
    assert!(matches!(read_positions(body, price, quantity), Err(ConnectError::Transport { .. })));
}

#[test]
fn a_zero_holding_with_no_entry_price_is_read_as_zero() {
    let (price, quantity) = scales();
    let body = r#"{"assetPositions":[{"position":{"coin":"ETH","szi":"0"}}]}"#;
    let held = read_positions(body, price, quantity).expect("a flat holding was refused");
    assert_eq!(held[0].entry_price_scaled, 0);
}

/// A wrong address returns empty, which looks like a flat account.
#[test]
fn an_account_address_is_checked_before_a_connection_is_opened() {
    assert_eq!(
        normalised_address("0x00000000000000000000000000000000000000AA").as_deref(),
        Some("0x00000000000000000000000000000000000000aa")
    );
    assert_eq!(normalised_address("00000000000000000000000000000000000000aa"), None);
    assert_eq!(normalised_address("0xaa"), None);
    assert_eq!(normalised_address("0x0000000000000000000000000000000000000zzz"), None);
    assert_eq!(normalised_address(""), None);
}

#[test]
fn a_client_refuses_an_account_address_it_cannot_use() {
    let clock = SequenceClock::starting_at(0);
    let refused = RestClient::new(ClientSetup {
        endpoint: "https://api.hyperliquid.xyz",
        wallet_key: &Credential::new(vec![7u8; 32]),
        account_address: "not-an-address",
        price: Scale::new(2).expect("a scale"),
        quantity: Scale::new(3).expect("a scale"),
        timeout: Duration::from_secs(1),
        clock: Arc::new(clock),
        mainnet: true,
    });
    assert!(matches!(refused, Err(ConnectError::NotPermitted { .. })));
}

/// A cloid is 128 bits of hex, the same for the same identifier, another for another.
#[test]
fn a_platform_identifier_becomes_a_128_bit_hex_cloid() {
    let cloid = super::cloid_for("bl-dmxmvto5wwpz-0");
    assert_eq!(cloid.len(), 34);
    assert!(cloid.starts_with("0x"));
    assert!(cloid[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(cloid, super::cloid_for("bl-dmxmvto5wwpz-0"));
    assert_ne!(cloid, super::cloid_for("bl-dmxmvto5wwpz-1"));
}
