// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Publish and subscribe, inside one program.
//! `docs/concepts/event_sourcing.md` says why single-writer is a type rather than a rule.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::clock::Clock;

pub mod delivery;
pub use delivery::{Delivered, Publication, Refusal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(u64);

/// Proof that a subscription attached. Only the bus can construct one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub topic: String,
    pub attached_at_ns: i128,
    pub capacity: usize,
}

/// The right to publish on one topic. Single-writer is a type, not a rule: two writers make the order a race.
#[derive(Debug)]
pub struct TopicWriter {
    topic: String,
}

impl TopicWriter {
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusError {
    EmptyTopic,
    AlreadyClaimed { topic: String },
    ZeroCapacity,
    UnknownSubscription(SubscriptionId),
}

impl core::fmt::Display for BusError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyTopic => write!(f, "a topic must have a name"),
            Self::AlreadyClaimed { topic } => write!(f, "{topic} already has a writer"),
            Self::ZeroCapacity => write!(f, "a subscription with no capacity receives nothing"),
            Self::UnknownSubscription(id) => write!(f, "no subscription {id:?}"),
        }
    }
}

impl std::error::Error for BusError {}

struct Queue {
    topic: String,
    capacity: usize,
    held: VecDeque<Delivered>,
}

/// One routing table: two tables for one message could report success while reaching no subscriber.
pub struct Bus {
    clock: Arc<dyn Clock>,
    /// Ordered, so dispatch is deterministic. Hash map order depends on the build (rule 6).
    queues: BTreeMap<SubscriptionId, Queue>,
    claimed: BTreeMap<String, ()>,
    sequences: BTreeMap<String, u64>,
    next_id: u64,
}

impl Bus {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            queues: BTreeMap::new(),
            claimed: BTreeMap::new(),
            sequences: BTreeMap::new(),
            next_id: 0,
        }
    }

    pub fn claim(&mut self, topic: &str) -> Result<TopicWriter, BusError> {
        let topic = topic.trim();
        if topic.is_empty() {
            return Err(BusError::EmptyTopic);
        }
        if self.claimed.contains_key(topic) {
            return Err(BusError::AlreadyClaimed { topic: topic.to_string() });
        }
        self.claimed.insert(topic.to_string(), ());
        self.sequences.entry(topic.to_string()).or_insert(0);
        Ok(TopicWriter { topic: topic.to_string() })
    }

    /// Attach to a topic, and say when.
    /// The subscriber states capacity. An unbounded queue would turn a reported refusal into memory growth.
    pub fn subscribe(&mut self, topic: &str, capacity: usize) -> Result<Subscription, BusError> {
        let topic = topic.trim();
        if topic.is_empty() {
            return Err(BusError::EmptyTopic);
        }
        if capacity == 0 {
            return Err(BusError::ZeroCapacity);
        }

        let id = SubscriptionId(self.next_id);
        self.next_id += 1;
        self.queues.insert(
            id,
            Queue { topic: topic.to_string(), capacity, held: VecDeque::with_capacity(capacity) },
        );

        Ok(Subscription {
            id,
            topic: topic.to_string(),
            attached_at_ns: self.clock.now_ns(),
            capacity,
        })
    }

    /// Publish, and report what happened to every subscriber.
    /// A message that could not be delivered is reported, never discarded with a success.
    pub fn publish(&mut self, writer: &TopicWriter, payload: &[u8]) -> Publication {
        let sequence = self.sequences.entry(writer.topic.clone()).or_insert(0);
        *sequence += 1;
        let sequence = *sequence;
        let stamped_at_ns = self.clock.now_ns();

        let mut delivered = 0usize;
        let mut refused = Vec::new();

        for (id, queue) in self.queues.iter_mut() {
            if queue.topic != writer.topic {
                continue;
            }
            if queue.held.len() >= queue.capacity {
                refused.push(Refusal { subscription: *id, behind_by: queue.held.len() });
                continue;
            }
            queue.held.push_back(Delivered {
                topic: writer.topic.clone(),
                payload: payload.to_vec(),
                sequence,
                stamped_at_ns,
            });
            delivered += 1;
        }

        Publication { topic: writer.topic.clone(), sequence, stamped_at_ns, delivered, refused }
    }

    /// Take everything waiting for a subscription, oldest first.
    pub fn drain(&mut self, id: SubscriptionId) -> Result<Vec<Delivered>, BusError> {
        let queue = self.queues.get_mut(&id).ok_or(BusError::UnknownSubscription(id))?;
        Ok(queue.held.drain(..).collect())
    }

    pub fn pending(&self, id: SubscriptionId) -> Result<usize, BusError> {
        self.queues.get(&id).map(|q| q.held.len()).ok_or(BusError::UnknownSubscription(id))
    }

    pub fn subscribers(&self, topic: &str) -> usize {
        self.queues.values().filter(|q| q.topic == topic).count()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/bus/tests.rs"]
mod tests;
