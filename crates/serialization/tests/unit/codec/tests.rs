// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Gates for the envelope and the primitives.
//! Covers what every class inherits: envelope refusals, bounds, and the trailing-byte rule.

use super::*;

#[test]
fn the_envelope_round_trips() {
    for class in ClassId::ALL {
        let payload = Writer::new(class, 1).finish();
        let (envelope, reader) = Reader::open(&payload).expect("a bare envelope opens");
        assert_eq!(envelope.class, class);
        assert_eq!(envelope.version, 1);
        reader.finish().expect("a class with no fields is fully consumed");
    }
}

/// An unknown class is refused, and the refusal names what it saw.
/// Skipping an unknown class in a limit-state replay silently restores budget.
#[test]
fn an_unknown_class_is_refused_not_skipped() {
    let payload = vec![0xff, 0xff, 0x00, 0x01];
    assert_eq!(Reader::open(&payload).unwrap_err(), DecodeError::UnknownClass { found: 0xffff });
}

#[test]
fn an_envelope_shorter_than_four_bytes_is_truncated_not_guessed() {
    for len in 0..ENVELOPE_LEN {
        let payload = vec![0u8; len];
        assert!(matches!(Reader::open(&payload), Err(DecodeError::Truncated { .. })));
    }
}

/// Trailing bytes are an error, not an allowance.
/// Otherwise two payloads decode alike and the byte-identical round trip means nothing.
#[test]
fn bytes_left_after_the_layout_are_refused() {
    let mut w = Writer::new(ClassId::LimitConsumed, 1);
    w.i128(42);
    let mut payload = w.finish();
    payload.push(0x00);

    let (_, mut r) = Reader::open(&payload).unwrap();
    assert_eq!(r.i128().unwrap(), 42);
    assert_eq!(r.finish().unwrap_err(), DecodeError::TrailingBytes { remaining: 1 });
}

#[test]
fn every_integer_width_round_trips_including_the_edges() {
    let mut w = Writer::new(ClassId::LimitDeclared, 1);
    w.u8(u8::MAX)
        .u16(u16::MAX)
        .u32(u32::MAX)
        .u64(u64::MAX)
        .i64(i64::MIN)
        .i128(i128::MIN)
        .i128(i128::MAX);

    let payload = w.finish();
    let (_, mut r) = Reader::open(&payload).unwrap();
    assert_eq!(r.u8().unwrap(), u8::MAX);
    assert_eq!(r.u16().unwrap(), u16::MAX);
    assert_eq!(r.u32().unwrap(), u32::MAX);
    assert_eq!(r.u64().unwrap(), u64::MAX);
    assert_eq!(r.i64().unwrap(), i64::MIN);
    assert_eq!(r.i128().unwrap(), i128::MIN);
    assert_eq!(r.i128().unwrap(), i128::MAX);
    r.finish().unwrap();
}

#[test]
fn strings_round_trip_and_an_empty_one_is_a_value() {
    let mut w = Writer::new(ClassId::LimitDeclared, 1);
    w.str("venue-a").unwrap().str("").unwrap();
    let payload = w.finish();

    let (_, mut r) = Reader::open(&payload).unwrap();
    assert_eq!(r.str().unwrap(), "venue-a");
    assert_eq!(r.str().unwrap(), "", "an empty name is empty, not absent");
    r.finish().unwrap();
}

#[test]
fn a_field_longer_than_the_bound_is_refused_on_the_way_in() {
    let mut w = Writer::new(ClassId::LimitDeclared, 1);
    let big = vec![0u8; MAX_FIELD_LEN as usize + 1];
    assert!(matches!(w.bytes(&big), Err(writer::EncodeError::FieldTooLong { .. })));
}

/// And on the way out, because the length on disk is a claim rather than a fact.
#[test]
fn a_corrupt_length_is_refused_before_anything_is_allocated() {
    let mut payload = Writer::new(ClassId::LimitDeclared, 1).finish();
    payload.extend_from_slice(&0xffff_ffffu32.to_be_bytes());

    let (_, mut r) = Reader::open(&payload).unwrap();
    assert_eq!(r.bytes().unwrap_err(), DecodeError::FieldTooLong { found: 0xffff_ffff });
}

#[test]
fn a_short_read_names_what_it_wanted() {
    let mut payload = Writer::new(ClassId::LimitDeclared, 1).finish();
    payload.push(0x01);

    let (_, mut r) = Reader::open(&payload).unwrap();
    assert_eq!(r.i128().unwrap_err(), DecodeError::Truncated { wanted: 16, remaining: 1 });
}

#[test]
fn invalid_utf8_is_refused_rather_than_replaced() {
    let mut w = Writer::new(ClassId::LimitDeclared, 1);
    w.bytes(&[0xff, 0xfe]).unwrap();
    let payload = w.finish();

    let (_, mut r) = Reader::open(&payload).unwrap();
    // Lossy conversion here would turn a corrupt venue name into a plausible one.
    assert_eq!(r.str().unwrap_err(), DecodeError::NotUtf8);
}

/// Class ids are stable, and this test is the reason they stay that way.
/// Renumbering would reinterpret every event already stored, so the wire values are pinned.
#[test]
fn class_ids_are_pinned_to_their_wire_values() {
    assert_eq!(ClassId::LimitDeclared.as_u16(), 0x0101);
    assert_eq!(ClassId::LimitConsumed.as_u16(), 0x0102);
    assert_eq!(ClassId::LimitReleased.as_u16(), 0x0103);

    for class in ClassId::ALL {
        assert_eq!(ClassId::from_u16(class.as_u16()), Some(class));
    }
}

/// Encoding the same values twice produces the same bytes.
/// Binding under the event model. A rule with no test is a hope.
#[test]
fn encoding_is_deterministic() {
    let build = || {
        let mut w = Writer::new(ClassId::LimitConsumed, 1);
        w.str("desk-1").unwrap().str("venue-a").unwrap().i128(9_999);
        w.finish()
    };
    assert_eq!(build(), build());
}
