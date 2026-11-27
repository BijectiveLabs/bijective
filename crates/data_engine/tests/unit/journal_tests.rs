// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The journal codec: every entry round-trips, and the same entry always encodes alike.

use super::*;

#[test]
fn every_entry_round_trips_and_encodes_the_same_way_twice() {
    let entries = [
        JournalEntry::Frame {
            received_at_ns: 1_787_469_688_200_000_000,
            frame: b"{\"op\":\"pong\"}".to_vec(),
        },
        JournalEntry::Frame { received_at_ns: 0, frame: Vec::new() },
        JournalEntry::ContinuityLost { at_ns: 17 },
        JournalEntry::SessionStarted { at_ns: -1 },
        JournalEntry::SessionEnded { at_ns: i128::MAX },
    ];
    for entry in entries {
        let first = encode(&entry).expect("encode");
        assert_eq!(first, encode(&entry).expect("encode"));
        assert_eq!(decode(&first).expect("decode"), entry);
    }
}

#[test]
fn an_entry_of_an_unknown_kind_is_refused() {
    let mut bytes = encode(&JournalEntry::ContinuityLost { at_ns: 1 }).expect("encode");
    bytes[4] = 9;
    assert!(matches!(decode(&bytes), Err(DecodeError::InvalidValue { field: "entry", found: 9 })));
}

#[test]
fn a_payload_of_another_class_is_refused() {
    let link = crate::link::encode(&crate::link::LinkTransition {
        state: bijective_venue::contract::ConnectionState::Disconnected,
        at_ns: 1,
    })
    .expect("encode");
    assert!(matches!(decode(&link), Err(DecodeError::UnknownClass { .. })));
}
