// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Orders a previous process placed, taken back from the record at start. The venue knows an
//! order only by the hash of its signed content. The record fixes every signed field but the
//! exchange, which the outcome's book chose, so each order is held under the hash it signs to on
//! every exchange until the venue's evidence names one of them. Nothing is requested: an outcome's
//! book may be gone once its market resolved, and the hashes alone are enough to recognise it.

use bijective_venue::contract::Recorded;

use super::{Placed, RestClient};
use crate::order::{Maker, candidate_ids};
use crate::signing::Exchange;

impl RestClient {
    /// Holds every recorded order this process has not signed itself. A request that cannot be
    /// hashed is skipped: its submit was refused before any call, so the venue holds no order.
    pub fn adopt(&mut self, recorded: &[Recorded]) {
        for order in recorded {
            let client_id = &order.request.client_id;
            if self.hash_of.contains_key(client_id) || self.unresolved.contains_key(client_id) {
                continue;
            }
            let Some(hashes) = self.hashes_of(order) else { continue };
            for hash in &hashes {
                let placed = Placed { client_id: client_id.clone(), settled: order.terminal };
                self.placed.insert(hash.clone(), placed);
            }
            if let [hash] = hashes.as_slice() {
                self.hash_of.insert(client_id.clone(), hash.clone());
            } else {
                self.unresolved.insert(client_id.clone(), hashes);
            }
        }
    }

    /// The hash the venue named, or every hash the order could carry.
    fn hashes_of(&self, order: &Recorded) -> Option<Vec<String>> {
        if let Some(hash) = &order.venue_order_id {
            return Some(vec![hash.to_ascii_lowercase()]);
        }
        // The exchange is ignored: `candidate_ids` signs for each in turn.
        let maker = Maker {
            address: self.maker,
            signature_type: self.signature_type,
            exchange: Exchange::Ctf,
        };
        candidate_ids(&order.request, &maker, order.intent_ns).ok().map(Vec::from)
    }

    /// The venue named `hash`. When it is one of a restored order's candidates, it is that order's
    /// hash and the others are dropped: they name no order and are never asked about again.
    pub(super) fn resolve(&mut self, hash: &str) {
        let hash = hash.to_ascii_lowercase();
        let Some(client_id) = self.placed.get(&hash).map(|p| p.client_id.clone()) else { return };
        let Some(candidates) = self.unresolved.remove(&client_id) else { return };
        for other in candidates.iter().filter(|c| **c != hash) {
            self.placed.remove(other);
        }
        self.hash_of.insert(client_id, hash);
    }
}
