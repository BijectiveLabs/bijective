// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

#[test]
fn a_transition_round_trips_and_encodes_the_same_way_twice() {
    let states = [
        ConnectionState::Disconnected,
        ConnectionState::Connecting,
        ConnectionState::Connected { since: Moment::NotTracked },
        ConnectionState::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::Backlogged { queued: 4_096 },
        },
        ConnectionState::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::Stale { last_message: Moment::NotTracked },
        },
        ConnectionState::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::SubscriptionsIncomplete { attached: 0, requested: 40 },
        },
    ];

    for state in states {
        let shape = LinkTransition { state, at_ns: 1_700 };
        let first = encode(&shape).expect("encode");
        assert_eq!(first, encode(&shape).expect("encode"));
        assert_eq!(decode(&first).expect("decode"), shape);
    }
}

/// The session carries the real counts. A refusal across forty instruments once reported one.
#[test]
fn the_counts_survive_the_round_trip() {
    let shape = LinkTransition {
        state: ConnectionState::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::SubscriptionsIncomplete { attached: 3, requested: 40 },
        },
        at_ns: 9,
    };

    let back = decode(&encode(&shape).expect("encode")).expect("decode");
    match back.state {
        ConnectionState::Degraded {
            reason: DegradedReason::SubscriptionsIncomplete { attached, requested },
            ..
        } => assert_eq!((attached, requested), (3, 40)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn each_venue_opens_its_own_link_stream() {
    use bijective_event_store::store::StreamWriter;

    let dir = std::env::temp_dir().join(format!("bl-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");

    assert_ne!(stream("venue-a"), stream("venue-b"));
    let first = StreamWriter::open(&dir, &stream("venue-a")).expect("the store refused a name");
    let second = StreamWriter::open(&dir, &stream("venue-b")).expect("the store refused a name");

    drop(first);
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}
