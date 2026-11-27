// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The simulated venue behind the execution contract, so the order path cannot tell it from a
//! live one and a research run exercises the same code that places capital.

use bijective_model::{AccountSnapshot, Clearance, OrderRequest, PositionSnapshot, VenueSymbol};
use bijective_venue::contract::{Answer, ConnectError, VenueEvidence, VenueExecution};
use bijective_venue::normalise::VenueOutcome;
use bijective_venue::profile::CapabilityDeclaration;

use crate::engine::Engine;
use crate::model::SimulationModel;

pub struct SimulatedVenue {
    declaration: CapabilityDeclaration,
    engine: Engine,
}

impl SimulatedVenue {
    /// The declaration is the real venue's, so admission and validation use its catalogue.
    pub fn new(declaration: CapabilityDeclaration, model: SimulationModel) -> Self {
        Self { declaration, engine: Engine::new(model) }
    }

    /// A sandbox venue: its fills move `account` and a buy it cannot pay for is refused.
    pub fn with_account(
        declaration: CapabilityDeclaration,
        model: SimulationModel,
        account: crate::account::Account,
    ) -> Self {
        Self { declaration, engine: Engine::with_account(model, account) }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Market input and the clock are the caller's: it drives the book, then settles at its time.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Positions as a venue displays them. The entry price is the venue's figure, the open cost
    /// divided by the quantity and truncated toward zero, the way venues round what they display.
    /// The exact cost stays in the lots.
    pub fn account_now(&self, now_ns: i128) -> Result<AccountSnapshot, ConnectError> {
        let mut positions = Vec::new();
        for (instrument, quantity) in self.engine.ledger().open_positions() {
            let cost = self.engine.ledger().position(instrument).map(|p| p.open_cost_scaled());
            let Some(Ok(cost)) = cost else {
                return Err(ConnectError::Transport {
                    detail: "open cost exceeds 128 bits".into(),
                });
            };
            positions.push(PositionSnapshot {
                instrument: instrument.clone(),
                quantity_scaled: quantity,
                entry_price_scaled: cost / quantity.abs(),
            });
        }
        Ok(AccountSnapshot { observed_at_ns: now_ns, positions, margin: None })
    }
}

impl VenueExecution for SimulatedVenue {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    /// The clearance is the order path's proof. The simulated venue, like a live one, does not
    /// re-judge risk.
    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        order: &'a OrderRequest,
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        let outcome = self.engine.submit(&self.declaration, order, now_ns);
        Box::pin(async move { outcome })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        client_id: &'a str,
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        let outcome = self.engine.cancel(client_id, now_ns);
        Box::pin(async move { outcome })
    }

    fn account(&mut self, now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        let snapshot = self.account_now(now_ns);
        Box::pin(async move { snapshot })
    }

    /// The whole simulated venue, complete by construction.
    fn evidence(
        &mut self,
        _since_ns: i128,
        _now_ns: i128,
    ) -> Answer<'_, Result<VenueEvidence, ConnectError>> {
        let evidence = self.engine.evidence();
        Box::pin(async move { Ok(evidence) })
    }
}
