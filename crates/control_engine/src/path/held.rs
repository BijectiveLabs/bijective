// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Linked proposals on the order path (decision 0013): held, then released through the same
//! admission and risk gates as any proposal.

use std::collections::BTreeSet;

use bijective_limits::SafeState;
use bijective_model::Proposal;

use super::{ControlEvent, OrderPath, PathError, Placement};
use crate::linked::Release;

impl OrderPath {
    /// Records a linked proposal as held. It acts on the proposal's position, so the mark moves
    /// past it and a restart holds it rather than reading it again.
    pub fn hold(
        &mut self,
        proposal: &Proposal,
        from_position: u64,
        at_ns: i128,
    ) -> Result<Placement, PathError> {
        let event = ControlEvent::Held { proposal: proposal.clone(), from_position, at_ns };
        self.record(&event)?;
        Ok(Placement::Held)
    }

    /// Whether `proposal` may be held: `None`, or the reason it is refused, as an operator reads
    /// it. A child must reduce only and name a parent this record placed or holds. No proposal
    /// may reuse a held identifier or join a group a member of which has filled.
    #[must_use]
    pub fn unholdable(&self, proposal: &Proposal) -> Option<String> {
        let replayed = self.replayed.state()?;
        let (linked, orders) = (replayed.linked(), replayed.orders());
        if let Some(parent) = proposal.parent.as_deref() {
            if !proposal.reduce_only {
                return Some("a proposal with a parent reduces only (decision 0013)".into());
            }
            if !linked.knows(parent) {
                return Some(format!("the parent {parent} was neither placed nor held"));
            }
        }
        if linked.knows(&proposal.client_id) {
            return Some(format!("{} names a proposal already linked", proposal.client_id));
        }
        linked
            .group_filled(proposal, orders)
            .then(|| "a member of the group has already filled".to_string())
    }

    /// Every held proposal due an order now, with the quantity due. Empty when the record could
    /// not be replayed.
    #[must_use]
    pub fn releasable(&self) -> Vec<Release> {
        self.replayed.state().map(|r| r.linked().releasable(r.orders())).unwrap_or_default()
    }

    /// Orders a fill on another member of their group cancelled, for the sweep to withdraw.
    #[must_use]
    pub fn displaced_orders(&self) -> BTreeSet<String> {
        self.replayed.state().map(|r| r.linked().displaced_orders(r.orders())).unwrap_or_default()
    }

    /// The orders of `decision_id`'s group peers: alternatives, not added to it.
    pub(super) fn peer_orders(&self, decision_id: &str) -> BTreeSet<String> {
        self.replayed.state().map(|r| r.linked().peer_orders(decision_id)).unwrap_or_default()
    }

    /// Whether a held proposal may be released this pass. Not while the trading state refuses
    /// it, the limits are unrecovered or a flatten is in force: it stays held instead of being
    /// refused, so a halt does not end the protection a child carries.
    #[must_use]
    pub fn may_release(&self, reduce_only: bool) -> bool {
        let reduction = reduce_only && self.trading.current() == SafeState::Reducing;
        let flattening = self.replayed.state().is_none_or(|r| r.orders().flatten_asked().is_some());
        (self.trading.admits_new_orders() || reduction) && self.limits.may_trade() && !flattening
    }

    /// Whether a release of `id` was refused already in this process. A restart tries again.
    #[must_use]
    pub fn release_refused(&self, id: &str) -> bool {
        self.release_refused.contains(id)
    }

    pub(crate) fn note_release_refused(&mut self, id: &str) {
        self.release_refused.insert(id.to_string());
    }
}
