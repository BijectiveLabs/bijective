// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

fn rules(venue: &str, tick: i128) -> VenueInstruments {
    VenueInstruments {
        venue: venue.into(),
        instruments: vec![
            InstrumentRules {
                instrument: "btc-perp".into(),
                price_scale: 1,
                quantity_scale: 3,
                price_tick: tick,
                quantity_lot: 1,
                minimum_size: 1,
                minimum_notional: 50,
                trading: true,
                kind: Some("perpetual".into()),
                quote_currency: Some("usdt".into()),
                settlement_currency: Some("usdt".into()),
                base_currency: Some("btc".into()),
            },
            InstrumentRules {
                instrument: "eth-perp".into(),
                price_scale: 2,
                quantity_scale: 2,
                price_tick: 1,
                quantity_lot: 1,
                minimum_size: 1,
                minimum_notional: 500,
                trading: false,
                kind: None,
                quote_currency: None,
                settlement_currency: None,
                base_currency: None,
            },
        ],
    }
}

#[test]
fn the_rules_round_trip_and_encode_the_same_way_twice() {
    let declared = rules("venue-a", 5);
    let bytes = encode(&declared).expect("encode");
    assert_eq!(bytes, encode(&declared).expect("encode"));
    assert_eq!(decode(&bytes).expect("decode"), declared);
}

#[test]
fn the_latest_declaration_for_a_venue_stands() {
    let root = std::env::temp_dir().join(format!("bl-instruments-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    assert_eq!(standing(&root, "venue-a").expect("read"), None);

    for declared in [rules("venue-a", 5), rules("venue-b", 1), rules("venue-a", 10)] {
        let mut w =
            bijective_event_store::store::StreamWriter::open(&root, &stream_for(&declared.venue))
                .expect("open");
        w.append(&encode(&declared).expect("encode")).expect("append");
    }
    assert_eq!(standing(&root, "venue-a").expect("read"), Some(rules("venue-a", 10)));
    assert_eq!(standing(&root, "venue-b").expect("read"), Some(rules("venue-b", 1)));
}

/// A declaration written before kinds and currencies were kept is read with them absent, so a
/// node upgraded in place reads its own history and declares again only because rules changed.
#[test]
fn a_version_1_declaration_is_read_with_kind_and_currencies_absent() {
    let mut w = crate::codec::Writer::new(ClassId::VenueInstrumentsDeclared, 1);
    w.str("venue-a").expect("venue");
    w.u64(1);
    w.str("eth-perp").expect("instrument");
    w.u8(2).u8(2);
    w.i128(1).i128(1).i128(1).i128(500);
    w.u8(0);
    let read = decode(&w.finish()).expect("a version 1 declaration");
    assert_eq!(read.instruments, rules("venue-a", 1).instruments[1..].to_vec());
}

/// A version 2 declaration carries kind and currencies but not the base, which is read absent.
#[test]
fn a_version_2_declaration_is_read_with_the_base_absent() {
    let mut w = crate::codec::Writer::new(ClassId::VenueInstrumentsDeclared, 2);
    w.str("venue-a").expect("venue");
    w.u64(1);
    w.str("btc-perp").expect("instrument");
    w.u8(1).u8(3);
    w.i128(5).i128(1).i128(1).i128(50);
    w.u8(1);
    for text in ["perpetual", "usdt", "usdt"] {
        w.u8(1).str(text).expect("text");
    }
    let read = decode(&w.finish()).expect("a version 2 declaration");
    let mut expected = rules("venue-a", 5).instruments[0].clone();
    expected.base_currency = None;
    assert_eq!(read.instruments, vec![expected]);
}
