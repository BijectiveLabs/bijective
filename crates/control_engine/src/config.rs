// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What an operator declares to the Control Engine, and what is refused before it takes effect.
//! Configuration assurance: unset safety settings are refused, not defaulted.

use std::collections::{BTreeMap, BTreeSet};

use bijective_common::configuration::{
    Invalid, Reason, distinct, known, non_empty, positive, required,
};
use bijective_limits::{Amount, LimitKind, Scope, ScopeLevel, Threshold};
use bijective_model::InstrumentId;
use bijective_risk::desks::{DEFAULT_COLLAR_BPS, Mandate, MandateBook, WIDEST_COLLAR_BPS};
use bijective_risk::valuation::MEASURED_LEVELS;
use serde::Deserialize;

mod assets;
pub use assets::Assets;
use assets::assets_of;
mod limits;
mod valuation;
use limits::limits_of;
use valuation::reporting_of;
pub use valuation::{RateDeclaration, RateMarket, ReportingCurrency, ValuationDeclaration};

/// The version this build reads. A newer file is refused, never partly applied.
pub const CONFIGURATION_VERSION: u32 = 1;

/// Wide integers travel as text: a JSON number can lose low digits in transit.
type Wide = String;

#[derive(Debug, Clone, Deserialize)]
pub struct DeskDeclaration {
    pub name: String,
    pub instruments: Vec<String>,
    pub venues: Vec<String>,
    pub maximum_per_order_scaled: Wide,
    /// Basis points a limit price may cross the mark by. Absent is the platform default.
    #[serde(default)]
    pub price_collar_bps: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LimitDeclaration {
    /// `portfolio`, `desk:<name>`, `venue:<name>`, `strategy:<name>`, `underlying:<asset>` or
    /// `instrument:<name>`.
    pub scope: String,
    pub kind: String,
    /// For a money kind, at the money scale.
    #[serde(default)]
    pub maximum_money_scaled: Option<Wide>,
    /// For a ratio kind (`leverage`, `margin_use`), where 10000 is one: 30000 is three times.
    #[serde(default)]
    pub maximum_ratio_scaled: Option<Wide>,
}

/// How this node reaches one account at one venue. The credential is named, never carried.
#[derive(Debug, Clone, Deserialize)]
pub struct VenueAccess {
    /// The account: its ledger, reconciliation, `venue:` limit scope and place in desks' lists.
    pub name: String,
    /// The registered venue the account is at, where it differs from the name (decision 0008).
    #[serde(default)]
    pub venue: Option<String>,
    pub endpoint: String,
    /// Venue assurance's profile for this venue, which is where the capability declaration comes from.
    pub profile: String,
    /// Keystore names, required by the venue's credential shape (`credential_shape` in the
    /// registry): an API key and secret, a wallet key and the account it trades for, or a wallet
    /// key with an API key, secret and passphrase, where `account_address` optionally names the
    /// smart wallet the key signs for. A name the shape does not use may be left out.
    #[serde(default)]
    pub key_name: String,
    #[serde(default)]
    pub secret_name: String,
    #[serde(default)]
    pub wallet_key_name: String,
    #[serde(default)]
    pub passphrase_name: String,
    /// Not a secret: the account a wallet key trades for, where the venue reads them apart.
    #[serde(default)]
    pub account_address: String,
    pub settle_coin: String,
    /// Retired by decision 0007: precision is read per instrument from the profile. Kept only so
    /// a configuration that still names one is refused with the reason, not silently obeyed.
    #[serde(default)]
    pub price_scale: Option<u8>,
    #[serde(default)]
    pub quantity_scale: Option<u8>,
    pub request_timeout_ns: Wide,
    /// At most this many submits to the venue in any window. No default, because the venue's own
    /// published limit decides it and a guessed one would be either unsafe or idle.
    pub order_rate: OrderRateDeclaration,
}

impl VenueAccess {
    /// The registered venue: the adapter, credential shape, market stream and fees it uses.
    #[must_use]
    pub fn dialect(&self) -> &str {
        self.venue.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderRateDeclaration {
    pub orders: u32,
    pub per_ns: Wide,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Configuration {
    pub version: u32,
    /// Which venues this node may reach. A desk cannot prefer one that is not here.
    pub venues: Vec<String>,
    /// One block per venue in `venues`, and no others.
    #[serde(default)]
    pub venue_access: Vec<VenueAccess>,
    pub desks: Vec<DeskDeclaration>,
    pub limits: Vec<LimitDeclaration>,
    /// Maximum age of an exposure observation. No default: unset is refused.
    pub freshness_bound_ns: Option<Wide>,
    /// The sandbox account a paper node starts with: per venue, per currency, money at eight
    /// decimals (`docs/decisions/0003-sandbox.md`). Read only in paper mode, where every
    /// configured venue must have one.
    #[serde(default)]
    pub sandbox: BTreeMap<String, BTreeMap<String, Wide>>,
    /// Absent: the book is held to one settlement currency, and a position in another is unvalued.
    #[serde(default)]
    pub valuation: Option<ValuationDeclaration>,
    /// Each platform asset and the venue base names that denote it. Absent, a base is its own
    /// asset (`docs/decisions/0006-asset-names.md`).
    #[serde(default)]
    pub assets: BTreeMap<String, Vec<String>>,
}

/// A checked configuration. Only `Configuration::accept` produces one.
#[derive(Debug, Clone)]
pub struct Accepted {
    mandates: MandateBook,
    access: Vec<VenueAccess>,
    limits: Vec<(Scope, LimitKind, Threshold)>,
    venues: Vec<String>,
    freshness_bound_ns: i128,
    order_rates: Vec<(String, crate::rate::OrderRate)>,
    sandbox: BTreeMap<String, BTreeMap<String, i128>>,
    reporting: Option<ReportingCurrency>,
    assets: Assets,
}

impl Accepted {
    /// Which venue base names denote one asset. Applied wherever a profile is read.
    #[must_use]
    pub fn assets(&self) -> &Assets {
        &self.assets
    }

    /// The declared reporting currency and rate markets, or `None` for a one-currency book.
    #[must_use]
    pub fn reporting(&self) -> Option<&ReportingCurrency> {
        self.reporting.as_ref()
    }

    /// Each venue's order rate, parsed and checked positive when the configuration was accepted.
    #[must_use]
    pub fn order_rates(&self) -> &[(String, crate::rate::OrderRate)] {
        &self.order_rates
    }

    /// The sandbox balances of `venue`, or `None` where none are declared.
    #[must_use]
    pub fn sandbox(&self, venue: &str) -> Option<&BTreeMap<String, i128>> {
        self.sandbox.get(venue)
    }

    #[must_use]
    pub fn mandates(&self) -> &MandateBook {
        &self.mandates
    }

    #[must_use]
    pub fn limits(&self) -> &[(Scope, LimitKind, Threshold)] {
        &self.limits
    }

    #[must_use]
    pub fn venues(&self) -> &[String] {
        &self.venues
    }

    /// How to reach each venue, in the order they were declared.
    #[must_use]
    pub fn access(&self) -> &[VenueAccess] {
        &self.access
    }

    #[must_use]
    pub fn freshness_bound_ns(&self) -> i128 {
        self.freshness_bound_ns
    }
}

fn wide(field: &str, text: &str) -> Result<i128, Invalid> {
    text.parse::<i128>()
        .map_err(|e| Invalid::new(field, Reason::Malformed { detail: e.to_string() }))
}

/// The configured spelling of a scope level, in the words this file reads.
const fn form(level: ScopeLevel) -> &'static str {
    match level {
        ScopeLevel::Portfolio => "portfolio",
        ScopeLevel::Desk => "desk:<name>",
        ScopeLevel::Venue => "venue:<name>",
        ScopeLevel::Strategy => "strategy:<name>",
        ScopeLevel::Underlying => "underlying:<asset>",
        ScopeLevel::Instrument => "instrument:<name>",
    }
}

/// Read a limit's scope. A level nothing measures is refused because it would halt the order path
/// on an unjudged limit every pass.
fn scope(field: &str, text: &str) -> Result<Scope, Invalid> {
    let refuse = || {
        let mut known: Vec<String> = MEASURED_LEVELS.iter().map(|l| form(*l).to_string()).collect();
        known.sort();
        Invalid::new(field, Reason::Unknown { value: text.to_string(), known })
    };

    let read = match text.split_once(':') {
        None if text == "portfolio" => Scope::Portfolio,
        Some(("desk", name)) => Scope::Desk(name.to_string()),
        Some(("venue", name)) => Scope::Venue(name.to_string()),
        Some(("strategy", name)) => Scope::Strategy(name.to_string()),
        // One asset whatever case it is written in, as the figures are keyed.
        Some(("underlying", asset)) if !asset.is_empty() => {
            Scope::Underlying(asset.to_ascii_lowercase())
        }
        Some(("instrument", name)) => Scope::Instrument(name.to_string()),
        _ => return Err(refuse()),
    };

    if MEASURED_LEVELS.contains(&read.level()) { Ok(read) } else { Err(refuse()) }
}

impl Configuration {
    pub fn parse(raw: &[u8]) -> Result<Self, Invalid> {
        serde_json::from_slice(raw).map_err(|e| {
            // Carry the parser's message so the operator can act on it.
            Invalid::new("configuration", Reason::Malformed { detail: e.to_string() })
        })
    }

    /// Checks every value before any arithmetic touches it.
    pub fn accept(self) -> Result<Accepted, Invalid> {
        check_version(self.version)?;
        check_venues(&self.venues, &self.venue_access)?;
        check_access(&self.venue_access, &self.venues)?;
        let freshness = required("freshness_bound_ns", self.freshness_bound_ns.as_deref())?;
        let freshness_bound_ns =
            positive("freshness_bound_ns", wide("freshness_bound_ns", freshness)?)?;
        let mandates = mandates_of(&self.desks, &self.venues)?;
        let limits = limits_of(&self.limits, &self.desks)?;
        let order_rates = order_rates_of(&self.venue_access)?;
        let sandbox = sandbox_of(&self.sandbox, &self.venues)?;
        let reporting =
            self.valuation.as_ref().map(|v| reporting_of(v, &self.venues)).transpose()?;
        Ok(Accepted {
            mandates,
            access: self.venue_access,
            limits,
            venues: self.venues,
            freshness_bound_ns,
            order_rates,
            sandbox,
            reporting,
            assets: assets_of(&self.assets)?,
        })
    }
}

/// Each balance positive, for a configured venue. A balance of nothing is no account.
fn sandbox_of(
    declared: &BTreeMap<String, BTreeMap<String, Wide>>,
    venues: &[String],
) -> Result<BTreeMap<String, BTreeMap<String, i128>>, Invalid> {
    let mut out = BTreeMap::new();
    for (venue, balances) in declared {
        known("sandbox", venue, venues)?;
        let mut parsed = BTreeMap::new();
        for (currency, amount) in balances {
            let field = format!("sandbox.{venue}.{currency}");
            parsed.insert(currency.clone(), positive(&field, wide(&field, amount)?)?);
        }
        out.insert(venue.clone(), parsed);
    }
    Ok(out)
}

fn order_rates_of(
    access: &[VenueAccess],
) -> Result<Vec<(String, crate::rate::OrderRate)>, Invalid> {
    access
        .iter()
        .map(|a| {
            let field = format!("venue_access.{}.order_rate", a.name);
            let orders = positive(&format!("{field}.orders"), i128::from(a.order_rate.orders))?;
            let per = wide(&format!("{field}.per_ns"), &a.order_rate.per_ns)?;
            let per_ns = positive(&format!("{field}.per_ns"), per)?;
            let orders = u32::try_from(orders).unwrap_or(u32::MAX);
            Ok((a.name.clone(), crate::rate::OrderRate { orders, per_ns }))
        })
        .collect()
}

fn check_version(version: u32) -> Result<(), Invalid> {
    if version != CONFIGURATION_VERSION {
        return Err(Invalid::new(
            "version",
            Reason::Unknown {
                value: version.to_string(),
                known: vec![CONFIGURATION_VERSION.to_string()],
            },
        ));
    }

    Ok(())
}

/// Every venue is named once and has a registered adapter. An unregistered venue would refuse
/// every order at run time with no visible cause.
fn check_venues(venues: &[String], access: &[VenueAccess]) -> Result<(), Invalid> {
    non_empty("venues", venues)?;
    distinct("venues", venues)?;
    for venue in venues {
        let dialect =
            access.iter().find(|a| &a.name == venue).map_or(venue.as_str(), |a| a.dialect());
        // A venue without a registered adapter would refuse every order at run time with no visible cause.
        known("venues", dialect, bijective_venue_registry::REGISTERED)?;
    }

    Ok(())
}

/// One complete access block per declared venue, and none for an undeclared one. Refused here
/// rather than at the first proposal.
/// The names the venue's credential needs, by the shape the build's registry gives it.
fn credential_names(access: &VenueAccess) -> Result<Vec<(&'static str, &String)>, Invalid> {
    use bijective_venue_registry::CredentialShape;

    let shape = bijective_venue_registry::credential_shape(access.dialect()).map_err(|_| {
        let known = bijective_venue_registry::REGISTERED.iter().map(|v| v.to_string()).collect();
        let value = access.dialect().to_string();
        Invalid::new("venue_access.venue", Reason::Unknown { value, known })
    })?;
    Ok(match shape {
        CredentialShape::KeyAndSecret => {
            vec![("key_name", &access.key_name), ("secret_name", &access.secret_name)]
        }
        CredentialShape::WalletKey => vec![
            ("wallet_key_name", &access.wallet_key_name),
            ("account_address", &access.account_address),
        ],
        CredentialShape::WalletKeyAndApiKey => vec![
            ("wallet_key_name", &access.wallet_key_name),
            ("key_name", &access.key_name),
            ("secret_name", &access.secret_name),
            ("passphrase_name", &access.passphrase_name),
        ],
    })
}

fn check_access(venue_access: &[VenueAccess], venues: &[String]) -> Result<(), Invalid> {
    let mut reached = BTreeSet::new();
    for access in venue_access {
        let field = format!("venue_access.{}", access.name);
        known("venue_access.name", &access.name, venues)?;
        if !reached.insert(access.name.clone()) {
            return Err(Invalid::new(
                "venue_access",
                Reason::Duplicate { value: access.name.clone() },
            ));
        }
        let fixed = [
            ("endpoint", &access.endpoint),
            ("profile", &access.profile),
            ("settle_coin", &access.settle_coin),
        ];
        for (name, value) in fixed.into_iter().chain(credential_names(access)?) {
            if value.trim().is_empty() {
                return Err(Invalid::new(format!("{field}.{name}"), Reason::Absent));
            }
        }
        for (name, given) in
            [("price_scale", access.price_scale), ("quantity_scale", access.quantity_scale)]
        {
            if given.is_some() {
                let detail = "precision is read per instrument from the profile (decision 0007); \
                              remove this field"
                    .to_string();
                return Err(Invalid::new(format!("{field}.{name}"), Reason::Malformed { detail }));
            }
        }
        positive(
            &format!("{field}.request_timeout_ns"),
            wide(&format!("{field}.request_timeout_ns"), &access.request_timeout_ns)?,
        )?;
    }

    // Refused here rather than at the first proposal.
    for venue in venues {
        if !reached.contains(venue) {
            return Err(Invalid::new(format!("venue_access.{venue}"), Reason::Absent));
        }
    }

    Ok(())
}

/// One mandate per desk: its instruments, its venue preference and its per-order maximum.
fn mandates_of(desks: &[DeskDeclaration], venues: &[String]) -> Result<MandateBook, Invalid> {
    let mut mandates = MandateBook::new();
    let mut named = BTreeSet::new();
    for desk in desks {
        let field = format!("desks.{}", desk.name);
        if !named.insert(desk.name.clone()) {
            return Err(Invalid::new("desks", Reason::Duplicate { value: desk.name.clone() }));
        }

        non_empty(&format!("{field}.instruments"), &desk.instruments)?;
        distinct(&format!("{field}.instruments"), &desk.instruments)?;
        non_empty(&format!("{field}.venues"), &desk.venues)?;
        distinct(&format!("{field}.venues"), &desk.venues)?;
        for venue in &desk.venues {
            // A preference for an unreachable venue would silently never apply.
            known(&format!("{field}.venues"), venue, venues)?;
        }

        let maximum = positive(
            &format!("{field}.maximum_per_order_scaled"),
            wide(&format!("{field}.maximum_per_order_scaled"), &desk.maximum_per_order_scaled)?,
        )?;

        let price_collar_bps = collar_of(&field, desk.price_collar_bps)?;
        mandates.declare(
            desk.name.clone(),
            Mandate {
                instruments: desk
                    .instruments
                    .iter()
                    .map(|i| InstrumentId::new(i.as_str()))
                    .collect(),
                venue_preference: desk.venues.clone(),
                maximum_per_order: Amount::from_scaled(maximum),
                price_collar_bps,
            },
        );
    }

    Ok(mandates)
}

/// A desk's collar: the default when unnamed, refused at zero (no limit price could cross at
/// all, which reads as a collar while refusing every marketable order) or beyond the widest.
fn collar_of(field: &str, declared: Option<u32>) -> Result<u32, Invalid> {
    let bps = declared.unwrap_or(DEFAULT_COLLAR_BPS);
    if bps == 0 || bps > WIDEST_COLLAR_BPS {
        return Err(Invalid::new(
            format!("{field}.price_collar_bps"),
            Reason::OutOfRange {
                found: i128::from(bps),
                minimum: 1,
                maximum: i128::from(WIDEST_COLLAR_BPS),
            },
        ));
    }
    Ok(bps)
}

/// A desk limit names a declared desk: a misspelt one would read as a limit in force and bind
/// nothing. A strategy has no declaration here, so its name is taken as written.
fn declared_desk(field: &str, scope: &Scope, desks: &[DeskDeclaration]) -> Result<(), Invalid> {
    let Scope::Desk(name) = scope else { return Ok(()) };
    if desks.iter().any(|d| &d.name == name) {
        return Ok(());
    }
    Err(Invalid::new(
        field,
        Reason::Unknown {
            value: name.clone(),
            known: desks.iter().map(|d| d.name.clone()).collect(),
        },
    ))
}

#[cfg(test)]
#[path = "../tests/unit/config/tests.rs"]
mod tests;
