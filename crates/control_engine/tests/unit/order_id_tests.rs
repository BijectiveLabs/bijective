// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The identifier sent to a venue: stable for one admission, distinct across admissions.

use crate::order_id::client_order_id;

#[test]
fn the_same_admission_always_gives_the_same_identifier() {
    assert_eq!(
        client_order_id(1_794_928_690_649_864_793, 7),
        client_order_id(1_794_928_690_649_864_793, 7)
    );
    assert_eq!(client_order_id(35, 36), "bl-z-10");
}

#[test]
fn another_position_or_another_instant_gives_another_identifier() {
    let at = 1_794_928_690_649_864_793;
    assert_ne!(client_order_id(at, 0), client_order_id(at, 1));
    assert_ne!(client_order_id(at, 0), client_order_id(at + 1, 0));
}

/// 36 characters is the narrowest limit a registered venue states.
#[test]
fn the_longest_identifier_fits_every_registered_venue() {
    let longest = client_order_id(i128::from(u64::MAX), u64::MAX);
    assert_eq!(longest.len(), 30);
    assert!(longest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
}

#[test]
fn an_instant_before_the_epoch_still_gives_an_identifier() {
    assert_eq!(client_order_id(-5, 3), "bl-0-3");
}
