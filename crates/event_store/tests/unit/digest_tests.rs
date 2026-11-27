// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Acceptance vectors for the digest, from FIPS 180-4 and its published examples.
//! A gate, not a test file: accepted an in-crate hash only against the published vectors.

use super::digest::{Digest, Sha256, sha256};

fn hex(d: Digest) -> String {
    d.to_hex()
}

#[test]
fn empty_input() {
    assert_eq!(
        hex(sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn abc() {
    assert_eq!(
        hex(sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn two_block_448_bit_message() {
    let m = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert_eq!(hex(sha256(m)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
}

#[test]
fn multi_block_896_bit_message() {
    let m = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";
    assert_eq!(hex(sha256(m)), "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1");
}

/// The long-message vector: one million 'a'.
/// Short vectors do not exercise the length counter past a single byte.
#[test]
fn one_million_a() {
    let mut s = Sha256::new();
    let chunk = [b'a'; 1000];
    for _ in 0..1000 {
        s.update(&chunk);
    }
    assert_eq!(hex(s.finish()), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
}

/// Feeding the same bytes in awkward pieces must give the same digest.
/// Chunking must not change the result, or the append path would silently corrupt the chain.
#[test]
fn chunking_does_not_change_the_result() {
    let msg: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let want = sha256(&msg);

    for split in [1usize, 63, 64, 65, 127, 128, 500] {
        let mut s = Sha256::new();
        for piece in msg.chunks(split) {
            s.update(piece);
        }
        assert_eq!(s.finish(), want, "chunked by {split}");
    }
}

/// Block-boundary lengths, where padding logic is most likely to be wrong.
/// 55 and 56 straddle where the length no longer fits the final block. 63/64/65 straddle the block.
#[test]
fn padding_boundaries_agree_with_incremental() {
    for len in [0usize, 1, 54, 55, 56, 57, 63, 64, 65, 119, 120, 128] {
        let msg = vec![0x5au8; len];
        let one_shot = sha256(&msg);
        let mut s = Sha256::new();
        s.update(&msg);
        assert_eq!(s.finish(), one_shot, "length {len}");
    }
}

#[test]
fn hex_round_trips() {
    let d = sha256(b"round trip");
    assert_eq!(Digest::from_hex(&d.to_hex()), Some(d));
}

#[test]
fn malformed_hex_is_refused_rather_than_guessed() {
    // A manifest may have been touched: parsing must fail, not improvise a digest.
    assert!(Digest::from_hex("").is_none());
    assert!(Digest::from_hex("ab").is_none());
    assert!(Digest::from_hex(&"g".repeat(64)).is_none());
    assert!(Digest::from_hex(&"A".repeat(64)).is_none(), "uppercase is not our form");
}

/// The chain's starting point is a real digest, not a run of zeroes.
#[test]
fn genesis_is_the_empty_digest() {
    assert_eq!(Digest::genesis(), sha256(b""));
    assert_ne!(Digest::genesis().0, [0u8; 32]);
}
