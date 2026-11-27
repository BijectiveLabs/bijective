// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Brings every amount into the book's one currency, at a rate read from a declared market, or
//! refuses it (`docs/decisions/0005-reporting-currency.md`).

use std::collections::BTreeMap;

use bijective_limits::{Amount, notional};
use bijective_model::InstrumentId;
use bijective_venue::profile::{Instrument, InstrumentKind};

use super::{Pricing, Unvalued};

/// Where one currency's rate is read, and how old it may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateSource {
    pub instrument: InstrumentId,
    /// A quote on a pegged pair can stand unchanged for a minute, so the bound is the rate's own.
    pub max_age_ns: i128,
}

/// A checked source: its market's price scale, from the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Checked {
    instrument: InstrumentId,
    price_scale: u8,
    max_age_ns: i128,
}

/// The node's reporting currency, and the market each other currency's rate to it is read from.
/// Only `Rates::declared` and `Rates::single` produce one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rates {
    reporting: Option<String>,
    sources: BTreeMap<String, Checked>,
}

static SINGLE: Rates = Rates { reporting: None, sources: BTreeMap::new() };

/// Why a declared rate market cannot carry the rate it is declared for. The node does not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateRefused {
    OwnCurrency { currency: String },
    NotCatalogued { currency: String, instrument: InstrumentId },
    NotSpot { currency: String, instrument: InstrumentId, kind: InstrumentKind },
    WrongPair { currency: String, instrument: InstrumentId, reporting: String },
    NoAgeBound { currency: String },
}

impl Rates {
    /// No reporting currency: the first position valued fixes the book's currency, and an amount
    /// in any other is unvalued.
    #[must_use]
    pub fn single() -> &'static Rates {
        &SINGLE
    }

    /// Each source must be a catalogued spot market whose base is its currency and whose quote is
    /// `reporting`, with a positive age bound. Currencies are compared and kept in lower case.
    pub fn declared(
        reporting: &str,
        sources: &BTreeMap<String, RateSource>,
        catalogue: &BTreeMap<InstrumentId, Instrument>,
    ) -> Result<Self, RateRefused> {
        let reporting = reporting.to_ascii_lowercase();
        let mut checked = BTreeMap::new();
        for (currency, source) in sources {
            let currency = currency.to_ascii_lowercase();
            let price_scale = check_source(&reporting, &currency, source, catalogue)?;
            let (instrument, max_age_ns) = (source.instrument.clone(), source.max_age_ns);
            checked.insert(currency, Checked { instrument, price_scale, max_age_ns });
        }
        Ok(Self { reporting: Some(reporting), sources: checked })
    }

    #[must_use]
    pub fn reporting(&self) -> Option<&str> {
        self.reporting.as_deref()
    }
}

/// The market's price scale, or why it cannot carry the rate.
fn check_source(
    reporting: &str,
    currency: &str,
    source: &RateSource,
    catalogue: &BTreeMap<InstrumentId, Instrument>,
) -> Result<u8, RateRefused> {
    let named = || (currency.to_string(), source.instrument.clone());
    if currency == reporting {
        return Err(RateRefused::OwnCurrency { currency: currency.to_string() });
    }
    if source.max_age_ns <= 0 {
        return Err(RateRefused::NoAgeBound { currency: currency.to_string() });
    }
    let Some(listed) = catalogue.get(&source.instrument) else {
        let (currency, instrument) = named();
        return Err(RateRefused::NotCatalogued { currency, instrument });
    };
    if listed.kind != InstrumentKind::Spot {
        let (currency, instrument) = named();
        return Err(RateRefused::NotSpot { currency, instrument, kind: listed.kind });
    }
    let base = listed.base_currency.as_deref().map(str::to_ascii_lowercase);
    if base.as_deref() != Some(currency) || !listed.quote_currency.eq_ignore_ascii_case(reporting) {
        let (currency, instrument) = named();
        return Err(RateRefused::WrongPair { currency, instrument, reporting: reporting.into() });
    }
    Ok(listed.precision.price_scale)
}

/// One pass's conversions into one currency.
pub struct Conversion<'a> {
    pricing: Pricing<'a>,
    currency: Option<String>,
}

impl<'a> Conversion<'a> {
    /// `currency` is the book's currency where an earlier stage fixed it. Otherwise the reporting
    /// currency, and failing that the first amount converted fixes it.
    #[must_use]
    pub fn new(pricing: Pricing<'a>, currency: Option<&str>) -> Self {
        let fixed = currency.or(pricing.rates.reporting()).map(str::to_ascii_lowercase);
        Self { pricing, currency: fixed }
    }

    #[must_use]
    pub fn pricing(&self) -> &Pricing<'a> {
        &self.pricing
    }

    #[must_use]
    pub fn currency(&self) -> Option<&str> {
        self.currency.as_deref()
    }

    /// `amount`, stated in `currency`, in the book's currency. `on` is the instrument the amount
    /// came from, named in the refusal. Rounded away from zero, so never smaller than exact.
    pub fn into_book(
        &mut self,
        amount: Amount,
        currency: &str,
        on: &InstrumentId,
    ) -> Result<Amount, Unvalued> {
        let currency = currency.to_ascii_lowercase();
        let book = match &self.currency {
            None => {
                self.currency = Some(currency);
                return Ok(amount);
            }
            Some(book) if *book == currency => return Ok(amount),
            Some(book) => book.clone(),
        };
        let Some(source) = self.pricing.rates.sources.get(&currency) else {
            return Err(Unvalued::ForeignSettlement {
                instrument: on.clone(),
                settles_in: currency,
                book_settles_in: book,
            });
        };
        let unrated = |age_ns: Option<i128>| Unvalued::NoRate {
            instrument: on.clone(),
            currency: currency.clone(),
            rate: source.instrument.clone(),
            age_ns,
        };
        let mark = self.pricing.marks.get(&source.instrument).filter(|m| m.price_scaled > 0);
        let mark = mark.ok_or_else(|| unrated(None))?;
        let age_ns = self.pricing.as_of_ns.saturating_sub(mark.observed_at_ns);
        if age_ns > source.max_age_ns {
            return Err(unrated(Some(age_ns)));
        }
        notional(amount.scaled(), Amount::SCALE, mark.price_scaled, source.price_scale)
            .map_err(|cause| Unvalued::Arithmetic { instrument: on.clone(), cause })
    }
}

impl core::fmt::Display for RateRefused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OwnCurrency { currency } => {
                write!(f, "a rate for {currency} is declared, which is the reporting currency")
            }
            Self::NotCatalogued { currency, instrument } => {
                write!(
                    f,
                    "the rate for {currency} is read from {instrument}, which no profile lists"
                )
            }
            Self::NotSpot { currency, instrument, kind } => write!(
                f,
                "the rate for {currency} is read from {instrument}, a {kind:?} market, not spot"
            ),
            Self::WrongPair { currency, instrument, reporting } => write!(
                f,
                "the rate for {currency} is read from {instrument}, which does not price \
                 {currency} in {reporting}"
            ),
            Self::NoAgeBound { currency } => {
                write!(f, "the rate for {currency} declares no positive max_age_ns")
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/valuation/currency_tests.rs"]
mod tests;
