// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order and request signatures against vectors computed independently with `eth-account`
//! 0.14 (EIP-712) and Python's `hmac` (request signature) on 2026-11-15, for the documented
//! typed data. The key is the public development key `0xac09...ff80`. It signs nothing real.

use super::*;

const KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const TOKEN: &str = "32338220190071351435772801779725302244575775216413325951443816017994629993401";

fn signer() -> Signer {
    let bytes: Vec<u8> =
        (0..32).map(|i| u8::from_str_radix(&KEY[2 * i..2 * i + 2], 16).expect("hex")).collect();
    Signer::from_secret(&bytes).expect("key")
}

fn order() -> UnsignedOrder {
    let maker = signer().address();
    UnsignedOrder {
        salt: 479_249_096_354,
        maker,
        signer: maker,
        token_id: uint256(TOKEN).expect("token"),
        maker_amount: 100_000_000,
        taker_amount: 50_000_000,
        side: 0,
        signature_type: 0,
        timestamp_ms: 1_780_449_126_930,
    }
}

#[test]
fn the_key_controls_the_documented_development_address() {
    assert_eq!(hex(&signer().address()), "f39fd6e51aad88f6f4ce6ab8827279cfffb92266");
}

#[test]
fn the_order_digest_and_signature_match_an_independent_signer_on_both_domains() {
    let cases = [
        (
            Exchange::Ctf,
            "7c4746ac963441cbae02a7132b80e7c06db1fb73e0fbfc941e4492d2cba7599a",
            "0xbc724e999754a8d2cf6bbdabbeb32532538ca593bfd535bcc5a6933c6da6536714bfd04622ed41c8b8b7cf2ce94f1d04c4235e382d666166d939b58f30d3ac971c",
        ),
        (
            Exchange::ProtocolV2,
            "2ffc26fcac2c4226d2a3c16cac8a990fb712b6b0ab398e03f16b1348f5832519",
            "0xd794e597892166d2bb2d4e014d51021a0f4b1dd90b77aece56cbbf32c9d55d6f7dbc4385e0da484c957fd34f91e7c9a2e41b20617bef0edbc12959dccaf9f1201c",
        ),
    ];
    for (exchange, digest, signature) in cases {
        let computed = order_digest(&order(), exchange);
        assert_eq!(hex(&computed), digest, "{exchange:?}");
        assert_eq!(signer().sign(&computed).expect("signed"), signature, "{exchange:?}");
    }
}

#[test]
fn a_decimal_wider_than_256_bits_or_not_decimal_is_refused() {
    assert_eq!(uint256("1").expect("one")[31], 1);
    assert!(uint256(&"9".repeat(80)).is_none());
    assert!(uint256("0x12").is_none());
    assert!(uint256("").is_none());
}

#[test]
fn the_request_signature_matches_an_independent_hmac() {
    let signature = request_signature(
        "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        1_700_000_000,
        "POST",
        "/order",
        r#"{"a":1}"#,
    )
    .expect("signed");
    assert_eq!(signature, "KuAtBdxlNSRO7yFe_5Qikip_BbnrGVoJBwVUQ47TuHA=");
    assert_eq!(
        request_signature("not base64!", 1, "GET", "/", ""),
        Err(SigningError::SecretRefused)
    );
}

/// EIP-55 examples and the development key, each checked with `eth_utils.to_checksum_address`.
#[test]
fn an_address_is_written_in_its_checksum_form() {
    for expected in [
        "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
        "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
        "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
        "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
    ] {
        let mut address = [0u8; 20];
        for (i, byte) in address.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&expected[2 + 2 * i..4 + 2 * i], 16).expect("hex");
        }
        assert_eq!(checksummed(&address), expected);
    }
    assert_eq!(checksummed(&signer().address()), "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
}

/// A Deposit Wallet order: maker and signer are the wallet, type 3. Hash and wrapped signature
/// computed independently with `eth-account` 0.14 (`encode_typed_data`, `TypedDataSign`) and
/// `eth_abi` on 2026-11-16, following the documented wrapping.
#[test]
fn a_deposit_wallet_signature_matches_an_independent_signer_on_both_domains() {
    let wallet = "2e234dae75c793f67a35089c9d99245e1c58470b";
    let mut address = [0u8; 20];
    for (i, byte) in address.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&wallet[2 * i..2 * i + 2], 16).expect("hex");
    }
    let order = UnsignedOrder { maker: address, signer: address, signature_type: 3, ..order() };
    let cases = [
        (
            Exchange::Ctf,
            "d2cd048844325e682f62d19c8d54fb7e363dc5b98b237553c8b1745289a51255",
            "5beb23a9080f439a9610d76dae708a2961fa7fb9e924d1621aae3349bd9b38c5709a356d8cc18d03584eab5360c4cfad14d99007ad2c733203361c5493c9dc9d1c",
            "3264e159346253e26a64e00b69032db0e7d32f94628de3e6eecb50304d7af3d2",
        ),
        (
            Exchange::ProtocolV2,
            "958b85a2d75ecf683f9f64cf0c8037db4cdd5802a3a5ce9b83b0aaef46f22420",
            "6cea98a836fc73dadae79b86cf7fba18d27ad640055cdd3205e467cd58c7c5c04d7bab79e6b44eead450d76524ba884522b1f289606905a14cd00436cecb2ea51b",
            "466c63910185bbd55e8679264200c4e0abdcbb0c6264eb3d41d13326022e095b",
        ),
    ];
    let contents = "d17f67836b7e994ee44fe40b1b48f500e1a1bb452d19c74d79512113491e93a5";
    for (exchange, order_hash, inner, separator) in cases {
        assert_eq!(hex(&order_digest(&order, exchange)), order_hash, "{exchange:?}");
        let expected = format!("0x{inner}{separator}{contents}{}00ba", hex(ORDER_TYPE.as_bytes()));
        let wrapped = deposit_wallet_signature(&signer(), &order, exchange).expect("signed");
        assert_eq!(wrapped, expected, "{exchange:?}");
    }
}
