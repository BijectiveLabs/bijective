// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A netted position per instrument, changed only by a fill or an audited adjustment.
//!
//! Open quantity is held as lots in the order they opened, each at its own price. A reduction
//! consumes the oldest lots first, and realized profit is the exact sum of (exit - entry) times
//! the quantity closed, so no average price is ever rounded into the record. A fill larger than
//! the position closes it and opens the remainder on the other side. Figures are scaled integers.
//! Realized profit is in quote units at the price scale plus the quantity scale, for linear
//! instruments.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use bijective_model::{Side, VenueSymbol};

/// Quantity open at one price. Positive. The direction is the position's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lot {
    pub price_scaled: i128,
    pub quantity_scaled: i128,
}

/// Why a position changed. The identity is unique across both kinds and applied once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionCause {
    Fill {
        fill_id: String,
    },
    /// A change not from trading (a transfer, a correction), recorded with who decided it and why.
    Adjustment {
        adjustment_id: String,
        reason: String,
    },
}

impl PositionCause {
    fn identity(&self) -> &str {
        match self {
            Self::Fill { fill_id } => fill_id,
            Self::Adjustment { adjustment_id, .. } => adjustment_id,
        }
    }
}

/// One change, for the audit record and for the reader that publishes `position_updated`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionChange {
    pub instrument: VenueSymbol,
    pub cause: PositionCause,
    pub quantity_before_scaled: i128,
    pub quantity_after_scaled: i128,
    pub realized_scaled: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionError {
    NotPositive {
        instrument: VenueSymbol,
        field: &'static str,
        value: i128,
    },
    /// A product or sum beyond `i128`. Refused, never wrapped or saturated.
    Overflow {
        instrument: VenueSymbol,
    },
    /// An identity already applied. The caller decides whether that is a duplicate or a defect.
    AlreadyApplied {
        instrument: VenueSymbol,
        identity: String,
    },
}

impl core::fmt::Display for PositionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotPositive { instrument, field, value } => {
                write!(f, "{instrument}: {field} {value} is not positive")
            }
            Self::Overflow { instrument } => write!(f, "{instrument}: a figure exceeds 128 bits"),
            Self::AlreadyApplied { instrument, identity } => {
                write!(f, "{instrument}: {identity} was already applied")
            }
        }
    }
}

impl std::error::Error for PositionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub instrument: VenueSymbol,
    /// Long when positive. Invariant: equals the lots' sum, signed by `direction`.
    quantity_scaled: i128,
    lots: VecDeque<Lot>,
    pub realized_scaled: i128,
    /// Fees by asset, each at its own scale. Never netted into profit across assets.
    pub fees: BTreeMap<(String, u8), i128>,
    /// Fills applied whose fee the venue did not state. Non-zero means `fees` is a lower bound,
    /// never the cost: an unstated fee is not a zero fee.
    pub fills_without_fee: u64,
    applied: BTreeSet<String>,
}

impl Position {
    pub fn flat(instrument: VenueSymbol) -> Self {
        Self {
            instrument,
            quantity_scaled: 0,
            lots: VecDeque::new(),
            realized_scaled: 0,
            fees: BTreeMap::new(),
            fills_without_fee: 0,
            applied: BTreeSet::new(),
        }
    }

    pub fn quantity_scaled(&self) -> i128 {
        self.quantity_scaled
    }

    pub fn lots(&self) -> impl Iterator<Item = &Lot> {
        self.lots.iter()
    }

    /// The exact cost of what is open, price times quantity summed over lots. A reader that wants
    /// an average divides this itself and owns the rounding.
    pub fn open_cost_scaled(&self) -> Result<i128, PositionError> {
        self.lots.iter().try_fold(0i128, |sum, lot| {
            lot.price_scaled
                .checked_mul(lot.quantity_scaled)
                .and_then(|c| sum.checked_add(c))
                .ok_or(self.overflow())
        })
    }

    pub fn has_applied(&self, identity: &str) -> bool {
        self.applied.contains(identity)
    }

    fn overflow(&self) -> PositionError {
        PositionError::Overflow { instrument: self.instrument.clone() }
    }

    /// Pure: returns the position after the change, or refuses and changes nothing.
    pub fn apply(
        &self,
        cause: PositionCause,
        side: Side,
        price_scaled: i128,
        quantity_scaled: i128,
    ) -> Result<(Position, PositionChange), PositionError> {
        for (field, value) in [("price", price_scaled), ("quantity", quantity_scaled)] {
            if value <= 0 {
                return Err(PositionError::NotPositive {
                    instrument: self.instrument.clone(),
                    field,
                    value,
                });
            }
        }
        if self.applied.contains(cause.identity()) {
            return Err(PositionError::AlreadyApplied {
                instrument: self.instrument.clone(),
                identity: cause.identity().to_string(),
            });
        }

        let mut next = self.clone();
        let signed = if side == Side::Buy { quantity_scaled } else { -quantity_scaled };
        let opposes =
            (self.quantity_scaled > 0 && signed < 0) || (self.quantity_scaled < 0 && signed > 0);
        let realized = if opposes { next.reduce(price_scaled, quantity_scaled)? } else { 0 };
        let consumed = if opposes { quantity_scaled.min(self.quantity_scaled.abs()) } else { 0 };
        let opening = quantity_scaled - consumed;
        if opening > 0 {
            next.lots.push_back(Lot { price_scaled, quantity_scaled: opening });
        }
        next.quantity_scaled = self.quantity_scaled.checked_add(signed).ok_or(self.overflow())?;
        next.realized_scaled = self.realized_scaled.checked_add(realized).ok_or(self.overflow())?;
        next.applied.insert(cause.identity().to_string());

        let change = PositionChange {
            instrument: self.instrument.clone(),
            cause,
            quantity_before_scaled: self.quantity_scaled,
            quantity_after_scaled: next.quantity_scaled,
            realized_scaled: realized,
        };
        Ok((next, change))
    }

    /// Consumes the oldest lots for up to `quantity`, returning the profit realized. Long positions
    /// gain when the exit is above the entry, short positions when it is below.
    fn reduce(&mut self, exit_price: i128, quantity: i128) -> Result<i128, PositionError> {
        let long = self.quantity_scaled > 0;
        let mut left = quantity;
        let mut realized = 0i128;
        while left > 0 {
            let Some(lot) = self.lots.front_mut() else { break };
            let closed = left.min(lot.quantity_scaled);
            let per_unit = if long {
                exit_price.checked_sub(lot.price_scaled)
            } else {
                lot.price_scaled.checked_sub(exit_price)
            };
            let gained = per_unit.and_then(|p| p.checked_mul(closed));
            let total = gained.and_then(|g| realized.checked_add(g));
            realized =
                total.ok_or(PositionError::Overflow { instrument: self.instrument.clone() })?;
            lot.quantity_scaled -= closed;
            left -= closed;
            if lot.quantity_scaled == 0 {
                self.lots.pop_front();
            }
        }
        Ok(realized)
    }

    /// A fill whose venue stated no fee: counted, so the fee totals read as incomplete.
    #[must_use]
    pub fn with_fee_unstated(mut self) -> Position {
        self.fills_without_fee += 1;
        self
    }

    /// Fees are recorded beside the position, in their own asset and scale.
    pub fn with_fee(
        mut self,
        asset: &str,
        scale: u8,
        amount_scaled: i128,
    ) -> Result<Position, PositionError> {
        let held = self.fees.entry((asset.to_string(), scale)).or_insert(0);
        *held = held
            .checked_add(amount_scaled)
            .ok_or(PositionError::Overflow { instrument: self.instrument.clone() })?;
        Ok(self)
    }
}
