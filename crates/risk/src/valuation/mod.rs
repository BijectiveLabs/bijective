// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Turns positions and prices into the figures risk limits judges.
//! `docs/concepts/risk_and_limits.md` covers why an unvalued position empties the snapshot.

use std::collections::BTreeMap;

use bijective_limits::{
    Amount, ArithmeticError, ExposureSnapshot, LimitKind, Scope, ScopeLevel, Threshold, notional,
};
use bijective_model::{HeldPosition, InstrumentId};
use bijective_venue::profile::{Instrument, InstrumentKind};

pub mod attribution;
pub mod currency;
pub mod margin;

use currency::{Conversion, Rates};

/// What instruments are priced with: the marks, the catalogue, and the rates into the book's
/// currency.
#[derive(Debug, Clone, Copy)]
pub struct Pricing<'a> {
    pub marks: &'a BTreeMap<InstrumentId, Mark>,
    pub catalogue: &'a BTreeMap<InstrumentId, Instrument>,
    pub rates: &'a Rates,
    /// The instant a rate's age is measured to, in UTC nanoseconds.
    pub as_of_ns: i128,
}

impl<'a> Pricing<'a> {
    /// Pricing for a book held to one settlement currency, with no rates.
    #[must_use]
    pub fn single(
        marks: &'a BTreeMap<InstrumentId, Mark>,
        catalogue: &'a BTreeMap<InstrumentId, Instrument>,
    ) -> Self {
        Self { marks, catalogue, rates: Rates::single(), as_of_ns: 0 }
    }
}

/// Levels this build produces a figure for. A limit at any other level is unjudged and risk limits stops.
/// Portfolio, venue, underlying and instrument come from what the venues hold (`value`). Desk and strategy
/// from the platform's placement record (`attribution::attributed`).
pub const MEASURED_LEVELS: [ScopeLevel; 6] = [
    ScopeLevel::Portfolio,
    ScopeLevel::Desk,
    ScopeLevel::Venue,
    ScopeLevel::Strategy,
    ScopeLevel::Underlying,
    ScopeLevel::Instrument,
];

/// The key an underlying is figured and limited under: the base currency, in lower case, because
/// venues and profiles write one currency in either case. `None` where none is declared.
#[must_use]
pub fn underlying_of(instrument: &Instrument) -> Option<String> {
    instrument.base_currency.as_deref().map(str::to_ascii_lowercase)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Running {
    gross: Amount,
    net: Amount,
}

impl Default for Running {
    fn default() -> Self {
        Self { gross: Amount::from_scaled(0), net: Amount::from_scaled(0) }
    }
}

impl Running {
    fn add(&mut self, magnitude: Amount, signed: Amount) -> Result<(), ArithmeticError> {
        self.gross = self.gross.add(magnitude)?;
        self.net = self.net.add(signed)?;
        Ok(())
    }

    fn figures(self, scope: Scope) -> [(Scope, LimitKind, Threshold); 2] {
        [
            (scope.clone(), LimitKind::GrossExposure, Threshold::Money(self.gross)),
            (scope, LimitKind::NetExposure, Threshold::Money(self.net)),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// In the instrument's own price scale.
    pub price_scaled: i128,
    pub observed_at_ns: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unvalued {
    NoMark {
        instrument: InstrumentId,
    },
    NotCatalogued {
        instrument: InstrumentId,
    },
    Arithmetic {
        instrument: InstrumentId,
        cause: ArithmeticError,
    },
    /// No rate into the book's currency is declared for the currency it settles in.
    ForeignSettlement {
        instrument: InstrumentId,
        settles_in: String,
        book_settles_in: String,
    },
    /// A rate is declared, but its market has no positive mark (`age_ns` absent) or one older
    /// than the rate's bound.
    NoRate {
        instrument: InstrumentId,
        currency: String,
        rate: InstrumentId,
        age_ns: Option<i128>,
    },
    /// A kind this platform does not value: an inverse contract is margined in the base asset, so
    /// quantity times price is wrong in the direction price moved, and a dated future has an expiry.
    UnsupportedKind {
        instrument: InstrumentId,
        kind: InstrumentKind,
    },
    /// Unknown, not nothing: a limit spanning the withdrawn venue comes back unjudged.
    VenueWithdrawn {
        venue: String,
    },
}

impl Unvalued {
    #[must_use]
    pub fn instrument(&self) -> Option<&InstrumentId> {
        Some(match self {
            Unvalued::NoMark { instrument }
            | Unvalued::NotCatalogued { instrument }
            | Unvalued::Arithmetic { instrument, .. }
            | Unvalued::ForeignSettlement { instrument, .. }
            | Unvalued::NoRate { instrument, .. }
            | Unvalued::UnsupportedKind { instrument, .. } => instrument,
            // A withdrawal is about a venue. A fabricated identity would be an invented figure.
            Unvalued::VenueWithdrawn { .. } => return None,
        })
    }
}

/// The venues a pass asked: those that answered, and those withdrawn, whose holdings are unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct Venues<'a> {
    pub read: &'a [String],
    pub withdrawn: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Valuation {
    pub snapshot: ExposureSnapshot,
    pub unvalued: Vec<Unvalued>,
    /// Profit not yet realised, at [`Amount::SCALE`], signed: the mark against each entry price.
    /// `None` whenever anything is unvalued, for the reason the snapshot is then empty.
    pub unrealized: Option<Amount>,
    /// The currency every figure is in. `None` when nothing fixed it (no reporting currency and
    /// nothing valued).
    pub currency: Option<String>,
}

/// One unvaluable position empties the snapshot, and the instant is the oldest input
/// (`docs/concepts/risk_and_limits.md`). A rate is judged against its own age bound instead.
#[must_use]
///
/// A venue read, an instrument catalogued and an underlying a catalogued instrument names has a
/// figure even when nothing is held there: zero is known, and a limit declared on it is judged.
pub fn value(
    positions: &[HeldPosition],
    pricing: &Pricing<'_>,
    venues: &Venues<'_>,
    positions_observed_at_ns: i128,
) -> Valuation {
    // Recorded first: a withdrawn venue has holdings this platform cannot see.
    let mut unvalued: Vec<Unvalued> = venues
        .withdrawn
        .iter()
        .map(|venue| Unvalued::VenueWithdrawn { venue: venue.clone() })
        .collect();
    let mut totals = Totals::listing(venues.read, pricing.catalogue);
    let mut oldest = positions_observed_at_ns;
    let mut unrealized = Amount::from_scaled(0);
    let mut book = Conversion::new(*pricing, None);

    for position in positions {
        let (instrument, mark) = match valuable(position, pricing) {
            Ok(inputs) => inputs,
            Err(why) => {
                unvalued.push(why);
                continue;
            }
        };
        oldest = oldest.min(mark.observed_at_ns);
        let signed = match exposure_of(position, instrument, mark, &mut book) {
            Ok(value) => value,
            Err(why) => {
                unvalued.push(why);
                continue;
            }
        };
        if let Err(why) = totals.add(position, underlying_of(instrument), signed) {
            unvalued.push(why);
        }
        match unrealized_in_book(position, instrument, mark, &mut book) {
            Ok(gain) => match unrealized.add(gain) {
                Ok(sum) => unrealized = sum,
                Err(cause) => unvalued
                    .push(Unvalued::Arithmetic { instrument: position.instrument.clone(), cause }),
            },
            Err(why) => unvalued.push(why),
        }
    }

    let complete = unvalued.is_empty();
    let figures = if complete { totals.figures() } else { Vec::new() };
    Valuation {
        snapshot: ExposureSnapshot { observed_at_ns: oldest, figures },
        unvalued,
        unrealized: complete.then_some(unrealized),
        currency: book.currency().map(str::to_string),
    }
}

/// The position's signed value at the mark, in the book's currency.
fn exposure_of(
    position: &HeldPosition,
    instrument: &Instrument,
    mark: &Mark,
    book: &mut Conversion<'_>,
) -> Result<Amount, Unvalued> {
    let signed = notional(
        position.quantity_scaled,
        instrument.precision.quantity_scale,
        mark.price_scaled,
        instrument.precision.price_scale,
    )
    .map_err(|cause| Unvalued::Arithmetic { instrument: position.instrument.clone(), cause })?;
    book.into_book(signed, &instrument.settlement_currency, &position.instrument)
}

/// The position's unrealised profit, in the book's currency.
fn unrealized_in_book(
    position: &HeldPosition,
    instrument: &Instrument,
    mark: &Mark,
    book: &mut Conversion<'_>,
) -> Result<Amount, Unvalued> {
    let gain = unrealized_on(position, instrument, mark)
        .map_err(|cause| Unvalued::Arithmetic { instrument: position.instrument.clone(), cause })?;
    book.into_book(gain, &instrument.settlement_currency, &position.instrument)
}

/// A realised figure as money. Realised profit is kept at the price scale plus the quantity scale.
pub fn realized_value(
    realized_scaled: i128,
    instrument: &Instrument,
) -> Result<Amount, ArithmeticError> {
    let scale = instrument.precision.price_scale + instrument.precision.quantity_scale;
    notional(realized_scaled, scale, 1, 0)
}

/// The portfolio's loss so far today: the day's result turned over. The result is what was
/// realised today plus the movement of the unrealised profit since the day opened.
pub fn daily_loss(
    realized_today: Amount,
    unrealized_now: Amount,
    unrealized_at_open: Amount,
) -> Result<(Scope, LimitKind, Threshold), ArithmeticError> {
    let result = realized_today.add(unrealized_now.sub(unrealized_at_open)?)?;
    let loss = Amount::from_scaled(0).sub(result)?;
    Ok((Scope::Portfolio, LimitKind::DailyLoss, Threshold::Money(loss)))
}

/// Mark against entry on the quantity held. A short gains when the mark falls.
fn unrealized_on(
    position: &HeldPosition,
    instrument: &Instrument,
    mark: &Mark,
) -> Result<Amount, ArithmeticError> {
    let moved = mark
        .price_scaled
        .checked_sub(position.entry_price_scaled)
        .ok_or(ArithmeticError::Overflow)?;
    notional(
        position.quantity_scaled,
        instrument.precision.quantity_scale,
        moved,
        instrument.precision.price_scale,
    )
}

/// The catalogue entry and mark a position is valued with, or why it cannot be valued: not
/// catalogued, no mark, or a kind this platform does not value.
fn valuable<'c>(
    position: &HeldPosition,
    pricing: &Pricing<'c>,
) -> Result<(&'c Instrument, &'c Mark), Unvalued> {
    let Some(instrument) = pricing.catalogue.get(&position.instrument) else {
        return Err(Unvalued::NotCatalogued { instrument: position.instrument.clone() });
    };
    let Some(mark) = pricing.marks.get(&position.instrument) else {
        return Err(Unvalued::NoMark { instrument: position.instrument.clone() });
    };
    if !instrument.kind.is_tradable() {
        return Err(Unvalued::UnsupportedKind {
            instrument: position.instrument.clone(),
            kind: instrument.kind,
        });
    }
    Ok((instrument, mark))
}

/// Running gross and net per level a venue's positions can be summed at. Desk and strategy need
/// the platform's own placement record, so `attribution` measures them.
#[derive(Default)]
struct Totals {
    book: Running,
    by_venue: BTreeMap<String, Running>,
    by_instrument: BTreeMap<InstrumentId, Running>,
    by_underlying: BTreeMap<String, Running>,
    /// A position whose instrument declares no base: every underlying figure could be short of
    /// it, so none is produced (`docs/decisions/0001-underlying-scope-level.md`).
    base_unknown: bool,
}

impl Totals {
    /// Every venue read, every catalogued instrument and every underlying they name, at zero.
    fn listing(venues: &[String], catalogue: &BTreeMap<InstrumentId, Instrument>) -> Self {
        let mut totals = Self::default();
        for venue in venues {
            totals.by_venue.entry(venue.clone()).or_default();
        }
        for (id, instrument) in catalogue {
            totals.by_instrument.entry(id.clone()).or_default();
            if let Some(asset) = underlying_of(instrument) {
                totals.by_underlying.entry(asset).or_default();
            }
        }
        totals
    }

    /// Gross is unsigned, net signed: a long and a short cancel in one and add in the other.
    fn add(
        &mut self,
        position: &HeldPosition,
        underlying: Option<String>,
        signed: Amount,
    ) -> Result<(), Unvalued> {
        let magnitude = Amount::from_scaled(signed.scaled().abs());
        let venue = self.by_venue.entry(position.venue.clone()).or_default();
        let instrument = self.by_instrument.entry(position.instrument.clone()).or_default();
        let asset = match underlying {
            Some(asset) => self.by_underlying.entry(asset).or_default().add(magnitude, signed),
            None => {
                self.base_unknown = true;
                Ok(())
            }
        };
        if self.book.add(magnitude, signed).is_err()
            || venue.add(magnitude, signed).is_err()
            || instrument.add(magnitude, signed).is_err()
            || asset.is_err()
        {
            return Err(Unvalued::Arithmetic {
                instrument: position.instrument.clone(),
                cause: ArithmeticError::Overflow,
            });
        }
        Ok(())
    }

    fn figures(self) -> Vec<(Scope, LimitKind, Threshold)> {
        let mut figures = Vec::new();
        figures.extend(self.book.figures(Scope::Portfolio));
        for (venue, running) in self.by_venue {
            figures.extend(running.figures(Scope::Venue(venue)));
        }
        for (instrument, running) in self.by_instrument {
            figures.extend(running.figures(Scope::Instrument(instrument.as_str().to_string())));
        }
        if !self.base_unknown {
            for (asset, running) in self.by_underlying {
                figures.extend(running.figures(Scope::Underlying(asset)));
            }
        }
        figures
    }
}

#[cfg(test)]
#[path = "../../tests/unit/valuation/tests.rs"]
mod tests;

impl core::fmt::Display for Unvalued {
    /// Names both currencies so an operator can act on the mismatch.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Unvalued::NoMark { instrument } => write!(f, "no mark for {instrument}"),
            Unvalued::NotCatalogued { instrument } => {
                write!(f, "{instrument} is not in the venue's catalogue")
            }
            Unvalued::Arithmetic { instrument, cause } => {
                write!(f, "{instrument} could not be valued: {cause:?}")
            }
            Unvalued::ForeignSettlement { instrument, settles_in, book_settles_in } => write!(
                f,
                "{instrument} settles in {settles_in}, and no rate into {book_settles_in}, the \
                 book's currency, is declared"
            ),
            Unvalued::NoRate { instrument, currency, rate, age_ns } => match age_ns {
                None => write!(f, "{instrument} settles in {currency}, and {rate} has no mark"),
                Some(age) => {
                    write!(f, "{instrument} settles in {currency}, and {rate} is {age} ns old")
                }
            },
            Unvalued::UnsupportedKind { instrument, kind } => {
                write!(
                    f,
                    "{instrument} is a {kind:?} contract, which this valuation does not price"
                )
            }
            Unvalued::VenueWithdrawn { venue } => {
                write!(f, "the venue {venue} is withdrawn")
            }
        }
    }
}
