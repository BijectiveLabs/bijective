// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Orders and positions together, so a fill changes both or neither.
//!
//! The ledger is a projection: built by applying events in order, it holds nothing that a replay
//! of the same events would not rebuild. Every refusal leaves it exactly as it was.

use std::collections::BTreeMap;

use bijective_model::{OrderRequest, VenueSymbol};

use crate::order::{Applied, LifecycleError, Order, OrderEvent, OrderState, Transition};
use crate::position::{Position, PositionCause, PositionChange, PositionError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// One client identifier, one order: a second creation would let two orders share an identity.
    DuplicateClientId {
        client_id: String,
    },
    UnknownClientId {
        client_id: String,
    },
    Lifecycle(LifecycleError),
    Position(PositionError),
}

impl core::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateClientId { client_id } => {
                write!(f, "{client_id} already names an order")
            }
            Self::UnknownClientId { client_id } => write!(f, "{client_id} names no order"),
            Self::Lifecycle(e) => write!(f, "{e}"),
            Self::Position(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// What one event did: the order's transition and, for a fill, the position's change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerChange {
    /// Boxed: the duplicate case carries nothing.
    Applied { transition: Box<Transition>, position: Option<Box<PositionChange>> },
    /// A fill delivered again. Nothing changed.
    Duplicate,
}

/// Counts an operator reads: how many orders may still execute, and how many are unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerSummary {
    pub orders: usize,
    pub may_execute: usize,
    pub unknown: usize,
    pub positions_open: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    orders: BTreeMap<String, Order>,
    positions: BTreeMap<VenueSymbol, Position>,
}

impl Ledger {
    pub fn order(&self, client_id: &str) -> Option<&Order> {
        self.orders.get(client_id)
    }

    pub fn position(&self, instrument: &VenueSymbol) -> Option<&Position> {
        self.positions.get(instrument)
    }

    /// Every order, in client identifier order.
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }

    /// Every position not flat, with its signed quantity, in instrument order.
    pub fn open_positions(&self) -> impl Iterator<Item = (&VenueSymbol, i128)> {
        self.positions.iter().map(|(i, p)| (i, p.quantity_scaled())).filter(|(_, q)| *q != 0)
    }

    pub fn orders_in(&self, state: OrderState) -> impl Iterator<Item = &Order> {
        self.orders.values().filter(move |o| o.state == state)
    }

    pub fn summary(&self) -> LedgerSummary {
        LedgerSummary {
            orders: self.orders.len(),
            may_execute: self.orders.values().filter(|o| o.state.may_execute()).count(),
            unknown: self.orders_in(OrderState::Unknown).count()
                + self.orders_in(OrderState::Reconciling).count(),
            positions_open: self.positions.values().filter(|p| p.quantity_scaled() != 0).count(),
        }
    }

    /// Precondition: the request cleared risk. This records it as `Created`.
    pub fn create(&mut self, request: OrderRequest) -> Result<(), LedgerError> {
        if self.orders.contains_key(&request.client_id) {
            return Err(LedgerError::DuplicateClientId { client_id: request.client_id });
        }
        self.orders.insert(request.client_id.clone(), Order::new(request));
        Ok(())
    }

    /// Applies one event. A fill moves the order and its instrument's position together. If either
    /// refuses, neither changes.
    pub fn apply(
        &mut self,
        client_id: &str,
        event: &OrderEvent,
        at_ns: i128,
    ) -> Result<LedgerChange, LedgerError> {
        let order = self
            .orders
            .get(client_id)
            .ok_or_else(|| LedgerError::UnknownClientId { client_id: client_id.to_string() })?;
        let (next, transition) = match order.apply(event, at_ns).map_err(LedgerError::Lifecycle)? {
            Applied::Duplicate => return Ok(LedgerChange::Duplicate),
            Applied::Transitioned { order, transition } => (*order, transition),
        };

        let position = match event {
            OrderEvent::Filled(fill) => {
                let instrument = next.request.instrument.clone();
                let held = self
                    .positions
                    .get(&instrument)
                    .cloned()
                    .unwrap_or_else(|| Position::flat(instrument.clone()));
                let cause = PositionCause::Fill { fill_id: fill.fill_id.clone() };
                let (mut moved, change) = held
                    .apply(cause, fill.side, fill.price_scaled, fill.quantity_scaled)
                    .map_err(LedgerError::Position)?;
                // The position moves either way: the venue said it executed, and leaving executed
                // quantity out of the record would understate exposure, the figure risk acts on.
                moved = match &fill.fee {
                    Some(fee) => moved
                        .with_fee(&fee.asset, fee.scale, fee.amount_scaled)
                        .map_err(LedgerError::Position)?,
                    None => moved.with_fee_unstated(),
                };
                Some((instrument, moved, change))
            }
            _ => None,
        };

        self.orders.insert(client_id.to_string(), next);
        let change = position.map(|(instrument, moved, change)| {
            self.positions.insert(instrument, moved);
            Box::new(change)
        });
        Ok(LedgerChange::Applied { transition, position: change })
    }

    /// The only change to a position that is not a fill, recorded with its identity and reason.
    pub fn adjust(
        &mut self,
        instrument: &VenueSymbol,
        cause: PositionCause,
        side: bijective_model::Side,
        price_scaled: i128,
        quantity_scaled: i128,
    ) -> Result<PositionChange, LedgerError> {
        let held = self
            .positions
            .get(instrument)
            .cloned()
            .unwrap_or_else(|| Position::flat(instrument.clone()));
        let (moved, change) = held
            .apply(cause, side, price_scaled, quantity_scaled)
            .map_err(LedgerError::Position)?;
        self.positions.insert(instrument.clone(), moved);
        Ok(change)
    }
}
