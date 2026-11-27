// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Desk register: which desks exist and what each is allocated. Design: desk admission.
//! Without it `Scope::Desk(String)` accepts any string, so a typo creates a desk with no capital.

pub mod admission;
pub mod collar;

pub use admission::{Admitted, Mandate, MandateBook, Refusal, admit};
pub use collar::{CollarMark, DEFAULT_COLLAR_BPS, WIDEST_COLLAR_BPS, within_collar};

use std::collections::BTreeMap;

use bijective_limits::{Amount, ArithmeticError, Entity, Scope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    EmptyName,
    DuplicateDesk {
        name: String,
    },
    /// Refused, not read as a desk that may not trade: almost always a configuration mistake.
    ZeroAllocation {
        name: String,
    },
    /// Refused: every desk could sit inside its own limit while the portfolio breaches.
    OverAllocated {
        desks: i128,
        portfolio: i128,
    },
    Arithmetic(ArithmeticError),
    UnknownDesk {
        name: String,
    },
    /// Capital with no desk sits outside every allocation but still counts against the portfolio.
    Unassigned,
    StillHoldsRisk {
        name: String,
    },
}

impl core::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "a desk must have a name"),
            Self::DuplicateDesk { name } => write!(f, "{name} is registered twice"),
            Self::ZeroAllocation { name } => write!(f, "{name} is allocated nothing"),
            Self::OverAllocated { desks, portfolio } => write!(
                f,
                "desks are allocated {desks} against a portfolio of {portfolio}; every desk could \
                 pass while the book breaches"
            ),
            Self::Arithmetic(_) => write!(f, "the allocations do not sum to a representable total"),
            Self::UnknownDesk { name } => write!(f, "{name} is not a registered desk"),
            Self::Unassigned => write!(f, "the entity names no desk and desks are in use"),
            Self::StillHoldsRisk { name } => write!(f, "{name} still holds risk"),
        }
    }
}

impl std::error::Error for RegisterError {}

/// Empty means an operator who runs one book. A register never supplied is refused a layer up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Register {
    portfolio: Amount,
    desks: BTreeMap<String, Amount>,
}

impl Register {
    pub fn single_book(portfolio: Amount) -> Self {
        Self { portfolio, desks: BTreeMap::new() }
    }

    pub fn new(
        portfolio: Amount,
        desks: impl IntoIterator<Item = (String, Amount)>,
    ) -> Result<Self, RegisterError> {
        let mut held: BTreeMap<String, Amount> = BTreeMap::new();
        let mut total = Amount::from_scaled(0);

        for (name, allocation) in desks {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(RegisterError::EmptyName);
            }
            if allocation.scaled() <= 0 {
                return Err(RegisterError::ZeroAllocation { name });
            }
            if held.contains_key(&name) {
                return Err(RegisterError::DuplicateDesk { name });
            }
            total = total.add(allocation).map_err(RegisterError::Arithmetic)?;
            held.insert(name, allocation);
        }

        if total.scaled() > portfolio.scaled() {
            return Err(RegisterError::OverAllocated {
                desks: total.scaled(),
                portfolio: portfolio.scaled(),
            });
        }

        Ok(Self { portfolio, desks: held })
    }

    pub fn is_single_book(&self) -> bool {
        self.desks.is_empty()
    }

    pub fn knows(&self, name: &str) -> bool {
        self.desks.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.desks.keys().map(String::as_str)
    }

    pub fn allocation(&self, name: &str) -> Option<Amount> {
        self.desks.get(name).copied()
    }

    /// Stated so a deliberate reserve is distinguishable from an accidental under-allocation.
    pub fn unallocated(&self) -> Amount {
        let allocated = self
            .desks
            .values()
            .try_fold(Amount::from_scaled(0), |acc, a| acc.add(*a))
            .unwrap_or(self.portfolio);
        self.portfolio.sub(allocated).unwrap_or(Amount::from_scaled(0))
    }

    /// Needed because `Scope::Desk` carries a bare `String`. A typo would read as a limit in force.
    pub fn admits(&self, scope: &Scope) -> Result<(), RegisterError> {
        match scope {
            Scope::Desk(name) if !self.knows(name) => {
                Err(RegisterError::UnknownDesk { name: name.clone() })
            }
            _ => Ok(()),
        }
    }

    /// Unassigned entities are refused, not parked on a catch-all (configuration assurance: absent is unsafe).
    pub fn admits_entity(&self, entity: &Entity) -> Result<(), RegisterError> {
        if self.is_single_book() {
            return Ok(());
        }
        if entity.desk.trim().is_empty() {
            return Err(RegisterError::Unassigned);
        }
        if !self.knows(entity.desk.trim()) {
            return Err(RegisterError::UnknownDesk { name: entity.desk.clone() });
        }
        Ok(())
    }

    /// `holds_risk` comes from the caller: this crate holds no position record.
    pub fn remove(&mut self, name: &str, holds_risk: bool) -> Result<(), RegisterError> {
        if !self.knows(name) {
            return Err(RegisterError::UnknownDesk { name: name.to_string() });
        }
        if holds_risk {
            return Err(RegisterError::StillHoldsRisk { name: name.to_string() });
        }
        self.desks.remove(name);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/desks/admission_tests.rs"]
mod admission_tests;
#[cfg(test)]
#[path = "../../tests/unit/desks/collar_tests.rs"]
mod collar_tests;
#[cfg(test)]
#[path = "../../tests/unit/desks/tests.rs"]
mod tests;
