// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One test per rule in and the determinism check section 11 makes a gate obligation.

use crate::clock::SequenceClock;

use super::*;

fn bus_at(now_ns: i128) -> (Bus, Arc<SequenceClock>) {
    let clock = Arc::new(SequenceClock::starting_at(now_ns));
    (Bus::new(clock.clone()), clock)
}

/// One table: a message reaches a topic's subscribers, or reaches no subscriber and reports it.
#[test]
fn a_message_reaches_its_topics_subscribers_and_no_others() {
    let (mut bus, _) = bus_at(0);
    let market = bus.claim("market").unwrap();
    let a = bus.subscribe("market", 4).unwrap();
    let b = bus.subscribe("market", 4).unwrap();
    let elsewhere = bus.subscribe("limits", 4).unwrap();

    let published = bus.publish(&market, b"a book update");

    assert_eq!(published.delivered, 2);
    assert!(published.reached_everyone());
    assert_eq!(bus.drain(a.id).unwrap().len(), 1);
    assert_eq!(bus.drain(b.id).unwrap().len(), 1);
    assert_eq!(bus.drain(elsewhere.id).unwrap().len(), 0, "another topic saw nothing");
}

/// A publication with no subscriber differs from one some subscriber missed.
#[test]
fn a_publication_with_no_subscribers_says_it_reached_nobody() {
    let (mut bus, _) = bus_at(0);
    let topic = bus.claim("market").unwrap();

    let published = bus.publish(&topic, b"no subscriber");
    assert!(!published.reached_anyone());
    assert!(published.reached_everyone(), "and no subscriber refused it either");
}

/// The confirmed-subscription rule: no way to hold a `Subscription` the bus did not register.
#[test]
fn subscribing_returns_proof_that_it_attached() {
    let (mut bus, _) = bus_at(1_700_000_000_000_000_000);
    let attached = bus.subscribe("market", 8).unwrap();

    assert_eq!(attached.attached_at_ns, 1_700_000_000_000_000_000);
    assert_eq!(attached.topic, "market");
    assert_eq!(bus.subscribers("market"), 1);
}

#[test]
fn a_subscription_with_no_capacity_is_refused_rather_than_created() {
    let (mut bus, _) = bus_at(0);
    assert_eq!(bus.subscribe("market", 0).unwrap_err(), BusError::ZeroCapacity);
    assert_eq!(bus.subscribe("  ", 4).unwrap_err(), BusError::EmptyTopic);
    assert_eq!(bus.subscribers("market"), 0);
}

/// An unreported discard is indistinguishable from a topic with no traffic.
#[test]
fn a_subscriber_that_cannot_keep_up_is_named_rather_than_dropped_quietly() {
    let (mut bus, _) = bus_at(0);
    let topic = bus.claim("market").unwrap();
    let slow = bus.subscribe("market", 2).unwrap();

    assert!(bus.publish(&topic, b"one").reached_everyone());
    assert!(bus.publish(&topic, b"two").reached_everyone());

    let overflowed = bus.publish(&topic, b"three");
    assert_eq!(overflowed.delivered, 0);
    assert!(!overflowed.reached_everyone(), "the publication does not report success");
    assert_eq!(overflowed.refused.len(), 1);
    assert_eq!(overflowed.refused[0].subscription, slow.id);
    assert_eq!(overflowed.refused[0].behind_by, 2, "how far behind, not merely that it is");
}

/// A subscriber that catches up receives again. A refusal is a state, not a verdict.
#[test]
fn a_subscriber_that_catches_up_receives_again() {
    let (mut bus, _) = bus_at(0);
    let topic = bus.claim("market").unwrap();
    let slow = bus.subscribe("market", 1).unwrap();

    bus.publish(&topic, b"one");
    assert!(!bus.publish(&topic, b"two").reached_everyone());

    let taken = bus.drain(slow.id).unwrap();
    assert_eq!(taken.len(), 1);
    assert!(bus.publish(&topic, b"three").reached_everyone());
}

/// The claim is a type, so a second writer cannot exist.
#[test]
fn a_topic_can_be_claimed_once() {
    let (mut bus, _) = bus_at(0);
    let _first = bus.claim("market").unwrap();

    assert_eq!(
        bus.claim("market").unwrap_err(),
        BusError::AlreadyClaimed { topic: "market".into() }
    );
}

#[test]
fn delivery_order_is_publication_order_and_carries_the_position() {
    let (mut bus, _) = bus_at(0);
    let topic = bus.claim("market").unwrap();
    let reader = bus.subscribe("market", 8).unwrap();

    for payload in [b"one".as_slice(), b"two", b"three"] {
        bus.publish(&topic, payload);
    }

    let taken = bus.drain(reader.id).unwrap();
    let order: Vec<&[u8]> = taken.iter().map(|d| d.payload.as_slice()).collect();
    assert_eq!(order, vec![b"one".as_slice(), b"two", b"three"]);
    assert_eq!(taken.iter().map(|d| d.sequence).collect::<Vec<_>>(), vec![1, 2, 3]);
}

/// Positions are per topic. A shared counter would make one topic's rate move another's numbers.
#[test]
fn each_topic_counts_its_own_positions() {
    let (mut bus, _) = bus_at(0);
    let market = bus.claim("market").unwrap();
    let limits = bus.claim("limits").unwrap();
    let m = bus.subscribe("market", 4).unwrap();
    let l = bus.subscribe("limits", 4).unwrap();

    bus.publish(&market, b"a");
    bus.publish(&market, b"b");
    bus.publish(&limits, b"c");

    assert_eq!(bus.drain(m.id).unwrap().last().unwrap().sequence, 2);
    assert_eq!(bus.drain(l.id).unwrap().last().unwrap().sequence, 1);
}

/// Driven by a sequence clock, the stamps are the sequence's, not the host's.
#[test]
fn stamps_come_from_the_clock_it_was_given() {
    let (mut bus, clock) = bus_at(500);
    let topic = bus.claim("market").unwrap();
    let reader = bus.subscribe("market", 4).unwrap();

    bus.publish(&topic, b"first");
    clock.advance_to(900).unwrap();
    bus.publish(&topic, b"second");

    let taken = bus.drain(reader.id).unwrap();
    assert_eq!(taken[0].stamped_at_ns, 500);
    assert_eq!(taken[1].stamped_at_ns, 900);
}

/// One publication sequence delivered twice must deliver identically. Byte-identical replay rests on it.
#[test]
fn one_publication_sequence_delivers_identically_twice() {
    fn run() -> Vec<(SubscriptionId, Vec<Delivered>)> {
        let (mut bus, clock) = bus_at(1_000);
        let market = bus.claim("market").unwrap();
        let limits = bus.claim("limits").unwrap();

        let readers: Vec<Subscription> = vec![
            bus.subscribe("market", 16).unwrap(),
            bus.subscribe("limits", 16).unwrap(),
            bus.subscribe("market", 16).unwrap(),
        ];

        for (n, payload) in [b"a".as_slice(), b"b", b"c", b"d"].iter().enumerate() {
            clock.advance_to(1_000 + n as i128 * 10).unwrap();
            bus.publish(if n % 2 == 0 { &market } else { &limits }, payload);
        }

        readers.iter().map(|r| (r.id, bus.drain(r.id).unwrap())).collect()
    }

    assert_eq!(run(), run());
}

/// The order subscriptions are visited must not depend on how the map happens to store them.
#[test]
fn subscribers_are_visited_in_a_stated_order() {
    let (mut bus, _) = bus_at(0);
    let topic = bus.claim("market").unwrap();

    let ids: Vec<SubscriptionId> = (0..8).map(|_| bus.subscribe("market", 1).unwrap().id).collect();

    // Every subscriber refuses. Refusals must come back in subscription order.
    bus.publish(&topic, b"fills them");
    let overflowed = bus.publish(&topic, b"refused by all");

    let refused: Vec<SubscriptionId> = overflowed.refused.iter().map(|r| r.subscription).collect();
    assert_eq!(refused, ids);
}
