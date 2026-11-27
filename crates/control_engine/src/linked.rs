// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Linked proposals as the control stream records them (decision 0013): a child held until its
//! parent fills, and one-cancels-other groups.
//!
//! The platform holds each link. Every leg is an ordinary order the node names, so
//! reconciliation reads no order it did not place. Everything here is derived from the record:
//! the same history always holds, releases and cancels the same legs.

use std::collections::{BTreeMap, BTreeSet};

use bijective_authority::Action;
use bijective_event_store::projection::Projection;
use bijective_model::Proposal;

use crate::orders::Orders;
use crate::wire::{ControlEvent, decode};

#[derive(Debug)]
pub enum LinkedError {
    Decode(bijective_serialization::codec::DecodeError),
}

impl std::fmt::Display for LinkedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkedError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

impl std::error::Error for LinkedError {}

/// A held proposal due an order: its quantity is what is still unreleased of what may be
/// released now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub proposal: Proposal,
    pub from_position: u64,
}

/// Where a held proposal stands, for a reader of the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldState {
    /// Some of it is still to be released.
    Waiting,
    /// All of it has been released as orders.
    Released,
    /// Another member of its group filled first: what is unreleased never will be.
    Displaced,
    /// An operator's cancel or flatten ended it before it was released in full.
    Ended,
}

/// A held proposal with what it has had released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holding {
    pub proposal: Proposal,
    pub released_scaled: i128,
    pub state: HoldState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    proposal: Proposal,
    from_position: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Linked {
    /// By the identifier the strategy proposed under.
    held: BTreeMap<String, Held>,
    /// Every proposal's orders, by its identifier, in the order their intents were recorded.
    orders_of: BTreeMap<String, Vec<String>>,
    /// What each held proposal's orders asked, summed: what it has had released.
    released: BTreeMap<String, i128>,
    /// Held proposals an operator's cancel or flatten ended before they were released in full.
    ended: BTreeSet<String>,
}

/// A group is scoped to its parent, so two brackets that use the same name never meet.
fn group_of(proposal: &Proposal) -> Option<(&str, &str)> {
    let oco = proposal.oco.as_deref()?;
    Some((proposal.parent.as_deref().unwrap_or(""), oco))
}

impl Linked {
    /// Whether `id` names a proposal this record placed an order for or holds.
    #[must_use]
    pub fn knows(&self, id: &str) -> bool {
        self.held.contains_key(id) || self.orders_of.contains_key(id)
    }

    /// What `id`'s orders have filled, summed.
    fn filled(&self, id: &str, orders: &Orders) -> i128 {
        self.client_ids(id).filter_map(|c| orders.order(c)).map(|o| o.filled_quantity_scaled).sum()
    }

    fn client_ids(&self, id: &str) -> impl Iterator<Item = &str> {
        self.orders_of.get(id).into_iter().flatten().map(String::as_str)
    }

    /// The other members of `id`'s group.
    fn peers_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a str> {
        let group = self.held.get(id).and_then(|h| group_of(&h.proposal));
        self.held
            .iter()
            .filter(move |(other, h)| {
                group.is_some() && *other != id && group_of(&h.proposal) == group
            })
            .map(|(other, _)| other.as_str())
    }

    /// Whether another member of `id`'s group has filled: `id` is then cancelled.
    fn displaced(&self, id: &str, orders: &Orders) -> bool {
        self.peers_of(id).any(|peer| self.filled(peer, orders) > 0)
    }

    /// Whether a member of the group `proposal` names has filled: a new member is refused.
    #[must_use]
    pub fn group_filled(&self, proposal: &Proposal, orders: &Orders) -> bool {
        let Some(group) = group_of(proposal) else { return false };
        self.held
            .iter()
            .filter(|(_, h)| group_of(&h.proposal) == Some(group))
            .any(|(id, _)| self.filled(id, orders) > 0)
    }

    /// Client identifiers of the orders of `decision_id`'s group peers: alternatives to it, so a
    /// reduce-only check does not count them against it.
    #[must_use]
    pub fn peer_orders(&self, decision_id: &str) -> BTreeSet<String> {
        self.peers_of(decision_id).flat_map(|p| self.client_ids(p)).map(str::to_string).collect()
    }

    /// Orders a fill on another member of their group cancelled.
    #[must_use]
    pub fn displaced_orders(&self, orders: &Orders) -> BTreeSet<String> {
        self.held
            .keys()
            .filter(|id| self.displaced(id, orders))
            .flat_map(|id| self.client_ids(id))
            .map(str::to_string)
            .collect()
    }

    /// What `id` may have released now and has not, or `None`.
    fn releasable_of(&self, id: &str, orders: &Orders) -> Option<i128> {
        let held = self.held.get(id)?;
        if self.ended.contains(id) || self.displaced(id, orders) {
            return None;
        }
        let quantity = held.proposal.quantity_scaled;
        let target = match held.proposal.parent.as_deref() {
            Some(parent) => quantity.min(self.filled(parent, orders)),
            None => quantity,
        };
        let due = target - self.released.get(id).copied().unwrap_or(0);
        (due > 0).then_some(due)
    }

    /// Every held proposal due an order, with the quantity due. Pure: the same record always
    /// releases the same quantities, so a restart releases nothing twice.
    #[must_use]
    pub fn releasable(&self, orders: &Orders) -> Vec<Release> {
        self.held
            .iter()
            .filter_map(|(id, held)| {
                let due = self.releasable_of(id, orders)?;
                let mut proposal = held.proposal.clone();
                proposal.quantity_scaled = due;
                Some(Release { proposal, from_position: held.from_position })
            })
            .collect()
    }

    /// Every proposal the record holds, by identifier, with where each stands.
    #[must_use]
    pub fn holdings(&self, orders: &Orders) -> Vec<Holding> {
        self.held
            .iter()
            .map(|(id, held)| {
                let released_scaled = self.released.get(id).copied().unwrap_or(0);
                let state = if released_scaled >= held.proposal.quantity_scaled {
                    HoldState::Released
                } else if self.ended.contains(id) {
                    HoldState::Ended
                } else if self.displaced(id, orders) {
                    HoldState::Displaced
                } else {
                    HoldState::Waiting
                };
                Holding { proposal: held.proposal.clone(), released_scaled, state }
            })
            .collect()
    }

    fn end_unreleased(&mut self) {
        let unreleased: Vec<String> = self
            .held
            .iter()
            .filter(|(id, h)| {
                self.released.get(*id).copied().unwrap_or(0) < h.proposal.quantity_scaled
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.ended.extend(unreleased);
    }
}

impl Projection for Linked {
    type Event = ControlEvent;
    type Error = LinkedError;

    fn decode(payload: &[u8]) -> Result<ControlEvent, LinkedError> {
        decode(payload).map_err(LinkedError::Decode)
    }

    fn apply(&mut self, event: &ControlEvent) -> Result<(), LinkedError> {
        match event {
            ControlEvent::Held { proposal, from_position, .. } => {
                let held = Held { proposal: proposal.clone(), from_position: *from_position };
                self.held.entry(proposal.client_id.clone()).or_insert(held);
            }
            ControlEvent::Intent { decision_id, request, .. } => {
                let ids = self.orders_of.entry(decision_id.clone()).or_default();
                ids.push(request.client_id.clone());
                if self.held.contains_key(decision_id) {
                    *self.released.entry(decision_id.clone()).or_insert(0) +=
                        request.quantity_scaled;
                }
            }
            // A cancel or a flatten ends what is still held, as it cancels what is working.
            ControlEvent::CommandApplied {
                action: Action::CancelOrders | Action::Flatten,
                detail,
                ..
            } if detail.is_empty() => self.end_unreleased(),
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/linked_tests.rs"]
mod tests;
