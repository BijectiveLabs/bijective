// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Exposure per desk and per strategy, from the platform's own placement record.
//!
//! A venue reports what it holds by instrument, never by who placed it, so a desk's or a
//! strategy's exposure is built from the fills of the orders it placed. Every figure is a bound
//! the true exposure cannot exceed, never an estimate: what its orders could still fill is
//! counted, and whatever the venue holds that the recorded fills do not explain (a fill not yet
//! read, a position taken outside the platform, an adjustment no owner was named on) is charged to
//! every desk and every strategy, because nothing says whose it is. `docs/concepts/risk_and_limits.md`.

use std::collections::{BTreeMap, BTreeSet};

use bijective_limits::{
    Amount, ArithmeticError, LimitKind, Scope, ScopeLevel, Threshold, notional,
};
use bijective_model::{HeldPosition, InstrumentId, Owner};

pub use super::Pricing;
use super::Unvalued;
use super::currency::Conversion;

/// One order this platform placed, or one adjustment a risk officer recorded for an owner, as
/// the record stands. Quantities are in the instrument's quantity scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// `None` on an order recorded before intents named their owner.
    pub owner: Option<Owner>,
    pub venue: String,
    pub instrument: InstrumentId,
    /// What its recorded fills bought, signed: negative is sold.
    pub filled_scaled: i128,
    /// What it could still buy, and still sell, while it may execute. Zero once it cannot.
    pub open_buy_scaled: i128,
    pub open_sell_scaled: i128,
}

/// The desk and strategy figures, and the oldest mark they were priced with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributed {
    pub figures: Vec<(Scope, LimitKind, Threshold)>,
    pub observed_at_ns: i128,
}

/// Gross and net exposure for every desk and strategy that placed an order, and for every desk
/// and strategy scope in `named`, which has a figure even when it placed nothing: a declared
/// limit without one would be unjudged.
///
/// Precondition: `held` is every position of every venue, read on this pass. A position or order
/// that cannot be priced (no mark, not catalogued, another settlement currency) produces no
/// figures at all, so every desk and strategy limit is unjudged rather than partly measured.
/// `book` carries the portfolio's currency, so both are measured in one.
pub fn attributed(
    held: &[HeldPosition],
    placed: &[Placed],
    book: &mut Conversion<'_>,
    named: &[Scope],
) -> Result<Attributed, Unvalued> {
    let mut priced = Priced { book, oldest: i128::MAX };
    let mut figures = Vec::new();
    for level in [ScopeLevel::Desk, ScopeLevel::Strategy] {
        let split = Split::of(level, held, placed);
        let mut keys: BTreeSet<String> = split.owned.keys().cloned().collect();
        keys.extend(named.iter().filter(|s| s.level() == level).map(name_of));
        for key in keys {
            let (gross, net) = split.exposure_of(&key, &mut priced)?;
            let scope = scope_at(level, key);
            figures.push((scope.clone(), LimitKind::GrossExposure, Threshold::Money(gross)));
            figures.push((scope, LimitKind::NetExposure, Threshold::Money(net)));
        }
    }
    Ok(Attributed { figures, observed_at_ns: priced.oldest })
}

// Quantities add saturating, never wrapping: a saturated quantity is the largest there is, so
// the money figure made from it overflows and is refused, and no figure is understated.

/// A venue and an instrument: where a quantity is held.
type Book = (String, InstrumentId);

#[derive(Debug, Default, Clone, Copy)]
struct Quantities {
    filled: i128,
    buy: i128,
    sell: i128,
}

/// What nobody at a level can be shown not to hold, charged to every key at it: the most it can
/// add to a gross figure, and how far it can move a net one up and down.
#[derive(Debug, Default, Clone, Copy)]
struct Charge {
    gross: i128,
    long: i128,
    short: i128,
}

/// The orders at one level, split into those a key owns and a charge for everything else.
struct Split {
    owned: BTreeMap<String, BTreeMap<Book, Quantities>>,
    charged: BTreeMap<Book, Charge>,
}

impl Split {
    fn of(level: ScopeLevel, held: &[HeldPosition], placed: &[Placed]) -> Self {
        let mut owned: BTreeMap<String, BTreeMap<Book, Quantities>> = BTreeMap::new();
        let mut charged: BTreeMap<Book, Charge> = BTreeMap::new();
        let mut explained: BTreeMap<Book, i128> = BTreeMap::new();
        for order in placed {
            let book = (order.venue.clone(), order.instrument.clone());
            let Some(key) = key_at(level, order.owner.as_ref()) else {
                // Its fills are in the venue's position and in no key's, so the residue below
                // holds them. What it could still fill is charged here.
                let charge = charged.entry(book).or_default();
                charge.gross =
                    charge.gross.saturating_add(order.open_buy_scaled.max(order.open_sell_scaled));
                charge.long = charge.long.saturating_add(order.open_buy_scaled);
                charge.short = charge.short.saturating_add(order.open_sell_scaled);
                continue;
            };
            let at = owned.entry(key).or_default().entry(book.clone()).or_default();
            at.filled = at.filled.saturating_add(order.filled_scaled);
            at.buy = at.buy.saturating_add(order.open_buy_scaled);
            at.sell = at.sell.saturating_add(order.open_sell_scaled);
            let sum = explained.entry(book).or_default();
            *sum = sum.saturating_add(order.filled_scaled);
        }
        let mut venue_held: BTreeMap<Book, i128> = BTreeMap::new();
        for position in held {
            let book = (position.venue.clone(), position.instrument.clone());
            let sum = venue_held.entry(book).or_default();
            *sum = sum.saturating_add(position.quantity_scaled);
        }
        let books: BTreeSet<Book> = venue_held.keys().chain(explained.keys()).cloned().collect();
        for book in books {
            let residue = venue_held
                .get(&book)
                .copied()
                .unwrap_or(0)
                .saturating_sub(explained.get(&book).copied().unwrap_or(0));
            let charge = charged.entry(book).or_default();
            charge.gross = charge.gross.saturating_add(residue.saturating_abs());
            charge.long = charge.long.saturating_add(residue.max(0));
            charge.short = charge.short.saturating_add(residue.min(0).saturating_neg());
        }
        Self { owned, charged }
    }

    /// The most `key` can hold, gross, and the net furthest from flat it can hold, summed over
    /// every book it or the charge touches.
    fn exposure_of(
        &self,
        key: &str,
        priced: &mut Priced<'_, '_>,
    ) -> Result<(Amount, Amount), Unvalued> {
        let empty = BTreeMap::new();
        let own = self.owned.get(key).unwrap_or(&empty);
        let books: BTreeSet<&Book> = own.keys().chain(self.charged.keys()).collect();
        let zero = Amount::from_scaled(0);
        let (mut gross, mut highest, mut lowest) = (zero, zero, zero);
        for book in books {
            let q = own.get(book).copied().unwrap_or_default();
            let charge = self.charged.get(book).copied().unwrap_or_default();
            // The position lies between all sells filling and all buys filling.
            let high = q.filled.saturating_add(q.buy);
            let low = q.filled.saturating_sub(q.sell);
            let gross_qty =
                high.saturating_abs().max(low.saturating_abs()).saturating_add(charge.gross);
            let high = high.saturating_add(charge.long);
            let low = low.saturating_sub(charge.short);
            if gross_qty == 0 && high == 0 && low == 0 {
                continue;
            }
            let overflow = |cause| Unvalued::Arithmetic { instrument: book.1.clone(), cause };
            gross = gross.add(priced.money(&book.1, gross_qty)?).map_err(overflow)?;
            highest = highest.add(priced.money(&book.1, high)?).map_err(overflow)?;
            lowest = lowest.add(priced.money(&book.1, low)?).map_err(overflow)?;
        }
        // A net limit is judged by magnitude, so the figure is whichever end is further from flat.
        let net = if lowest.scaled().saturating_abs() > highest.scaled().saturating_abs() {
            lowest
        } else {
            highest
        };
        Ok((gross, net))
    }
}

/// Prices quantities at the mark, in the book's currency, as the portfolio's valuation does.
struct Priced<'b, 'a> {
    book: &'b mut Conversion<'a>,
    oldest: i128,
}

impl Priced<'_, '_> {
    fn money(&mut self, id: &InstrumentId, quantity_scaled: i128) -> Result<Amount, Unvalued> {
        let pricing = *self.book.pricing();
        let Some(instrument) = pricing.catalogue.get(id) else {
            return Err(Unvalued::NotCatalogued { instrument: id.clone() });
        };
        let Some(mark) = pricing.marks.get(id) else {
            return Err(Unvalued::NoMark { instrument: id.clone() });
        };
        self.oldest = self.oldest.min(mark.observed_at_ns);
        let value = notional(
            quantity_scaled,
            instrument.precision.quantity_scale,
            mark.price_scaled,
            instrument.precision.price_scale,
        )
        .map_err(|cause: ArithmeticError| Unvalued::Arithmetic { instrument: id.clone(), cause })?;
        self.book.into_book(value, &instrument.settlement_currency, id)
    }
}

fn key_at(level: ScopeLevel, owner: Option<&Owner>) -> Option<String> {
    let owner = owner?;
    match level {
        ScopeLevel::Desk => Some(owner.desk.clone()),
        ScopeLevel::Strategy => owner.strategy.clone(),
        _ => None,
    }
}

fn scope_at(level: ScopeLevel, key: String) -> Scope {
    match level {
        ScopeLevel::Strategy => Scope::Strategy(key),
        _ => Scope::Desk(key),
    }
}

fn name_of(scope: &Scope) -> String {
    match scope {
        Scope::Desk(name)
        | Scope::Strategy(name)
        | Scope::Venue(name)
        | Scope::Underlying(name)
        | Scope::Instrument(name) => name.clone(),
        Scope::Portfolio => String::new(),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/valuation/attribution_tests.rs"]
mod tests;
