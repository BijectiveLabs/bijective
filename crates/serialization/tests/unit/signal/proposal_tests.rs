// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

fn proposal() -> Proposal {
    Proposal {
        desk: "alpha".into(),
        instrument: InstrumentId::new("btc-perp"),
        side: Side::Buy,
        quantity_scaled: 1_500,
        price_scaled: Some(200),
        cap_scaled: None,
        client_id: "abc123".into(),
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: false,
        parent: None,
        oco: None,
    }
}

/// The class round-trips, and encoding is stable. 's obligation.
#[test]
fn a_proposal_round_trips_and_encodes_the_same_way_twice() {
    let shapes = [
        proposal(),
        Proposal { side: Side::Sell, price_scaled: None, cap_scaled: Some(250), ..proposal() },
        Proposal { quantity_scaled: -1, price_scaled: None, cap_scaled: None, ..proposal() },
    ];

    for shape in &shapes {
        let first = encode(shape).expect("encode");
        assert_eq!(first, encode(shape).expect("encode"), "encoding must not vary");
        assert_eq!(&decode(&first).expect("decode"), shape);
    }
}

/// A payload from another domain is refused rather than read as ours.
#[test]
fn a_foreign_class_is_refused() {
    let mut w = Writer::new(ClassId::MarketBookUpdate, VERSION);
    w.i128(1);

    assert!(decode(&w.finish()).is_err());
}

/// A layout from a newer build is refused, never partially read.
#[test]
fn a_newer_version_is_refused() {
    let mut w = Writer::new(ClassId::ProposalEmitted, VERSION + 1);
    w.str("alpha").expect("write");

    assert!(matches!(decode(&w.finish()), Err(DecodeError::UnsupportedVersion { .. })));
}

/// A truncated payload is refused rather than filled in.
#[test]
fn a_truncated_payload_is_refused() {
    let full = encode(&proposal()).expect("encode");

    // Into the decided-at position, past the eight optional markers that may end a record.
    assert!(decode(&full[..full.len() - 12]).is_err());
}

/// A named venue round-trips. A record written before version 3 names none.
#[test]
fn a_named_venue_round_trips_and_an_older_record_names_none() {
    let mut named = proposal();
    named.venue = Some("venue-b".to_string());
    assert_eq!(decode(&encode(&named).expect("encode")).expect("decode"), named);

    let mut unnamed = named.clone();
    unnamed.venue = None;
    let mut bytes = encode(&unnamed).expect("encode");
    // The version-3 and version-4 tails of an unnamed venue and strategy are one absent marker
    // each. Without them and at version 2, the record is what an older build wrote.
    bytes.truncate(bytes.len() - 2);
    bytes[2..4].copy_from_slice(&2u16.to_be_bytes());
    assert_eq!(decode(&bytes).expect("a version 2 record"), unnamed);
}

/// The strategy round-trips. A record written before version 4 names none.
#[test]
fn a_named_strategy_round_trips_and_an_older_record_names_none() {
    let named =
        Proposal { strategy: Some("trend".into()), venue: Some("venue-b".into()), ..proposal() };
    assert_eq!(decode(&encode(&named).expect("encode")).expect("decode"), named);

    let unnamed = Proposal { strategy: None, ..named };
    let mut bytes = encode(&unnamed).expect("encode");
    // The strategy marker, the two terms bytes of version 5, version 6's stop and reduce flags and
    // version 7's two absent markers and version 8's expiry flag.
    bytes.truncate(bytes.len() - 8);
    bytes[2..4].copy_from_slice(&3u16.to_be_bytes());
    assert_eq!(decode(&bytes).expect("a version 3 record"), unnamed);
}

/// Terms round-trip. A record written before version 5 reads as the default terms.
#[test]
fn a_proposals_terms_round_trip_and_an_older_record_has_the_default() {
    let stated = Proposal {
        terms: bijective_model::OrderTerms {
            time_in_force: bijective_model::TimeInForce::ImmediateOrCancel,
            post_only: false,
            stop_price_scaled: Some(123),
            expire_at_ns: None,
        },
        ..proposal()
    };
    let bytes = encode(&stated).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), stated);
    let mut older = bytes.clone();
    // The two terms bytes, the stop (a flag and sixteen bytes), the reduce-only flag and the two
    // absent markers of version 7 and the expiry flag of version 8.
    older.truncate(older.len() - 23);
    older[2..4].copy_from_slice(&4u16.to_be_bytes());
    assert_eq!(decode(&older).expect("a version 4 record").terms, Default::default());
}

/// A reduce-only proposal keeps the flag. A version-5 record does not reduce only.
#[test]
fn reduce_only_round_trips_and_an_older_record_is_not_reduce_only() {
    let exit = Proposal { reduce_only: true, ..proposal() };
    let bytes = encode(&exit).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), exit);
    let mut older = bytes.clone();
    older.truncate(older.len() - 5);
    older[2..4].copy_from_slice(&5u16.to_be_bytes());
    assert!(!decode(&older).expect("a version 5 record").reduce_only);
}

/// A child's parent and group round-trip. A version-6 record names neither.
#[test]
fn a_parent_and_a_group_round_trip_and_an_older_record_names_neither() {
    let child = Proposal {
        reduce_only: true,
        parent: Some("entry-1".into()),
        oco: Some("bracket".into()),
        ..proposal()
    };
    let bytes = encode(&child).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), child);

    let unlinked = Proposal { parent: None, oco: None, ..child };
    let mut older = encode(&unlinked).expect("encode");
    older.truncate(older.len() - 3);
    older[2..4].copy_from_slice(&6u16.to_be_bytes());
    assert_eq!(decode(&older).expect("a version 6 record"), unlinked);
}

/// An expiry round-trips. A version-7 record does not expire (decision 0014).
#[test]
fn an_expiry_round_trips_and_an_older_record_does_not_expire() {
    let mut dated = proposal();
    dated.terms.expire_at_ns = Some(1_800_000_000_000_000_000);
    let bytes = encode(&dated).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), dated);
    let mut older = encode(&proposal()).expect("encode");
    older.pop();
    older[2..4].copy_from_slice(&7u16.to_be_bytes());
    assert_eq!(decode(&older).expect("a version 7 record"), proposal());
}
