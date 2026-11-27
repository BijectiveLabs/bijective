// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The valuation block: the reporting currency, its rate markets, and the streams read only for
//! rates (`docs/decisions/0005-reporting-currency.md`).

use std::collections::BTreeMap;

use bijective_common::configuration::{Invalid, Reason, distinct, positive, required};
use bijective_model::InstrumentId;
use bijective_risk::valuation::currency::RateSource;
use serde::Deserialize;

use super::{Wide, wide};

/// The node's reporting currency, and for each other currency the spot market its rate is read
/// from (`docs/decisions/0005-reporting-currency.md`).
#[derive(Debug, Clone, Deserialize)]
pub struct ValuationDeclaration {
    pub currency: String,
    /// Currency to the market pricing it in `currency`.
    #[serde(default)]
    pub rates: BTreeMap<String, RateDeclaration>,
    /// Market streams read only for rates, where a rate is not on a traded venue's own stream.
    #[serde(default)]
    pub markets: Vec<RateMarket>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateDeclaration {
    /// The platform identity of a spot market whose base is the currency.
    pub instrument: String,
    /// No default: how long a pegged pair's quote may stand unchanged is the operator's call.
    pub max_age_ns: Option<Wide>,
}

/// A market stream (`market-<name>`, written by a data engine run with `--stream-name <name>`)
/// and the profile its instruments are catalogued in. Nothing is traded on it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RateMarket {
    pub name: String,
    pub profile: String,
}

/// A checked valuation declaration, currencies in lower case. Whether each market can carry its
/// rate is checked against the catalogue when the venues open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingCurrency {
    pub currency: String,
    pub rates: BTreeMap<String, RateSource>,
    pub markets: Vec<RateMarket>,
}

fn named(field: &str, text: &str) -> Result<(), Invalid> {
    if text.trim().is_empty() { Err(Invalid::new(field, Reason::Empty)) } else { Ok(()) }
}

/// Every currency named and written once whatever its case, every rate market named with a
/// positive age bound, and every rate stream named once and apart from the venues.
pub(super) fn reporting_of(
    declared: &ValuationDeclaration,
    venues: &[String],
) -> Result<ReportingCurrency, Invalid> {
    named("valuation.currency", &declared.currency)?;
    let currencies: Vec<String> = declared.rates.keys().map(|c| c.to_ascii_lowercase()).collect();
    distinct("valuation.rates", &currencies)?;
    let mut rates = BTreeMap::new();
    for (currency, rate) in &declared.rates {
        let field = format!("valuation.rates.{currency}");
        named(&field, currency)?;
        named(&format!("{field}.instrument"), &rate.instrument)?;
        let age_field = format!("{field}.max_age_ns");
        let age = required(&age_field, rate.max_age_ns.as_deref())?;
        let max_age_ns = positive(&age_field, wide(&age_field, age)?)?;
        let instrument = InstrumentId::new(rate.instrument.clone());
        rates.insert(currency.to_ascii_lowercase(), RateSource { instrument, max_age_ns });
    }
    let streams: Vec<String> = declared.markets.iter().map(|m| m.name.clone()).collect();
    distinct("valuation.markets", &streams)?;
    for market in &declared.markets {
        named("valuation.markets.name", &market.name)?;
        named(&format!("valuation.markets.{}.profile", market.name), &market.profile)?;
        if venues.contains(&market.name) {
            let detail = format!("{} is a venue; its stream is read already", market.name);
            return Err(Invalid::new("valuation.markets", Reason::Malformed { detail }));
        }
    }
    let currency = declared.currency.to_ascii_lowercase();
    Ok(ReportingCurrency { currency, rates, markets: declared.markets.clone() })
}
