// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The profile, and the capability declaration that is derived from it.

use std::collections::BTreeMap;

use crate::profile::instrument::{CatalogueError, InstrumentCatalogue};
use bijective_model::VenueSymbol;
use serde::{Deserialize, Serialize};

use crate::profile::constraints::{IdentifierRules, Transformation};
use crate::profile::funding::{FundingModel, Transport};

pub const PROFILE_FORMAT: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DataClass {
    TopOfBook,
    Depth,
    Trades,
    Funding,
    MarkPrice,
    AccountState,
    OrderEvents,
    OpenInterest,
    Liquidations,
    IndexPrice,
    LongShortRatio,
    /// Which outcome of a prediction market won.
    Resolution,
}

/// The adapter contract: the declaration must say which. An emulated order type is a guarantee the venue
/// did not make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReduceOnly {
    PlacementPrecondition,
    /// Clamped at fill time - weaker than its name.
    FillTimeClamp,
    /// Not offered. Anything resembling it would be ours.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderTypeSupport {
    pub name: String,
    /// Native, or emulated by us. Only the first is the venue's guarantee.
    pub native: bool,
}

/// A sum type, not an optional: absent and none are different facts, and an unstated
/// rule would validate prices against ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "kebab-case")]
pub enum PriceValidity {
    IncrementOnly,
    /// At most this many significant figures, unless the price is an integer. The exception is the
    /// venue's own. Omitting it refuses prices the venue accepts.
    SignificantFigures {
        at_most: u8,
    },
}

impl PriceValidity {
    pub fn admits(self, scaled: i128, price_scale: u8) -> bool {
        let PriceValidity::SignificantFigures { at_most } = self else {
            return true;
        };
        if scaled == 0 {
            return true;
        }

        let unit = 10i128.saturating_pow(u32::from(price_scale));
        if scaled % unit == 0 {
            return true;
        }

        let mut magnitude = scaled.unsigned_abs();
        while magnitude.is_multiple_of(10) {
            magnitude /= 10;
        }
        u32::from(at_most) >= magnitude.to_string().len() as u32
    }
}

impl PrecisionRules {
    /// Whether `other` states the same rule, whatever scale each is written at: a profile may
    /// carry a venue's figures at the platform's scales, and 0.10 at scale 2 is 0.1 at scale 1.
    #[must_use]
    pub fn states_the_same_rule(&self, other: &PrecisionRules) -> bool {
        let same = |a: i128, scale_a: u8, b: i128, scale_b: u8| {
            let top = scale_a.max(scale_b);
            let widen = |v: i128, s: u8| {
                10i128.checked_pow(u32::from(top - s)).and_then(|f| v.checked_mul(f))
            };
            matches!((widen(a, scale_a), widen(b, scale_b)), (Some(x), Some(y)) if x == y)
        };
        self.price_validity == other.price_validity
            && same(self.price_tick, self.price_scale, other.price_tick, other.price_scale)
            && same(
                self.quantity_lot,
                self.quantity_scale,
                other.quantity_lot,
                other.quantity_scale,
            )
    }
}

/// A price off the tick is rejected at placement on live capital, so this is data, not a constant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecisionRules {
    pub price_scale: u8,
    pub quantity_scale: u8,
    /// Minimum price increment, in `price_scale` units.
    #[serde(with = "crate::profile::wide")]
    pub price_tick: i128,
    /// Minimum quantity increment, in `quantity_scale` units.
    #[serde(with = "crate::profile::wide")]
    pub quantity_lot: i128,
    pub price_validity: PriceValidity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncrementRejection {
    OffIncrement {
        value: i128,
        increment: i128,
    },
    /// The profile declares an increment of zero or less, which is not an increment.
    IncrementNotDeclared,
    TooManySignificantFigures {
        value: i128,
        at_most: u8,
    },
}

impl core::fmt::Display for IncrementRejection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IncrementRejection::OffIncrement { value, increment } => {
                write!(f, "{value} is not a multiple of the venue increment {increment}")
            }
            IncrementRejection::TooManySignificantFigures { value, at_most } => write!(
                f,
                "{value} carries more than {at_most} significant figures and is not an integer"
            ),
            IncrementRejection::IncrementNotDeclared => {
                write!(f, "the profile declares no usable increment")
            }
        }
    }
}

impl std::error::Error for IncrementRejection {}

impl PrecisionRules {
    /// Checked here rather than at placement: the venue's answer costs a round trip and arrives
    /// after the decision.
    pub fn check_price(&self, scaled: i128) -> Result<(), IncrementRejection> {
        check_increment(scaled, self.price_tick)?;
        if self.price_validity.admits(scaled, self.price_scale) {
            Ok(())
        } else {
            Err(IncrementRejection::TooManySignificantFigures {
                value: scaled,
                at_most: match self.price_validity {
                    PriceValidity::SignificantFigures { at_most } => at_most,
                    PriceValidity::IncrementOnly => 0,
                },
            })
        }
    }

    pub fn check_quantity(&self, scaled: i128) -> Result<(), IncrementRejection> {
        check_increment(scaled, self.quantity_lot)
    }
}

fn check_increment(value: i128, increment: i128) -> Result<(), IncrementRejection> {
    if increment <= 0 {
        return Err(IncrementRejection::IncrementNotDeclared);
    }
    if value % increment != 0 {
        return Err(IncrementRejection::OffIncrement { value, increment });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationStamp {
    #[serde(with = "crate::profile::wide")]
    pub at_ns: i128,
    pub verifier_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportProfile {
    pub funding: FundingModel,
    pub data_classes: Vec<DataClass>,
    /// The longest a figure from this transport may be relied on.
    #[serde(with = "crate::profile::wide::option")]
    pub max_staleness_ns: Option<i128>,
}

/// No `verified_at` means it was never verified, not that it has never drifted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueProfile {
    pub format: u16,
    pub venue: String,
    pub verified_at: Option<VerificationStamp>,
    pub transports: BTreeMap<Transport, TransportProfile>,
    pub reduce_only: ReduceOnly,
    pub order_types: Vec<OrderTypeSupport>,
    /// Each instrument carries its own precision. A venue-wide one was removed
    /// 2026-08-25.
    pub instruments: InstrumentCatalogue,
    pub identifier_rules: IdentifierRules,
    pub transformations: Vec<Transformation>,
    /// Carried by the profile, not a register the core must consult: an unread register is no
    /// register.
    pub support: crate::profile::support::SupportEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    UnsupportedFormat { found: u16 },
    SupportNamesAnotherVenue { profile: String, support: String },
    NoTransports,
    Malformed(String),
}

impl core::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProfileError::UnsupportedFormat { found } => {
                write!(f, "profile format {found} is newer than this build")
            }
            ProfileError::SupportNamesAnotherVenue { profile, support } => {
                write!(f, "profile for {profile} carries a support declaration for {support}")
            }
            ProfileError::NoTransports => write!(f, "profile declares no transport"),
            ProfileError::Malformed(why) => write!(f, "profile is malformed: {why}"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl VenueProfile {
    pub fn from_json(text: &str) -> Result<Self, ProfileError> {
        let p: VenueProfile =
            serde_json::from_str(text).map_err(|e| ProfileError::Malformed(e.to_string()))?;
        if p.format != PROFILE_FORMAT {
            return Err(ProfileError::UnsupportedFormat { found: p.format });
        }
        if p.transports.is_empty() {
            return Err(ProfileError::NoTransports);
        }
        // A full-support entry pasted from another venue must not dodge a data-only curation.
        if p.support.venue != p.venue {
            return Err(ProfileError::SupportNamesAnotherVenue {
                profile: p.venue.clone(),
                support: p.support.venue.clone(),
            });
        }
        Ok(p)
    }

    /// Pretty-printed because an operator reads it.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a profile is always serialisable")
    }
}

/// Derived, never authored: a declaration written beside a profile is a second source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDeclaration {
    venue: String,
    verified: bool,
    data_classes: Vec<DataClass>,
    native_order_types: Vec<String>,
    reduce_only: ReduceOnly,
    funding_by_transport: BTreeMap<Transport, FundingModel>,
    instruments: InstrumentCatalogue,
    #[serde(with = "crate::profile::wide::option")]
    max_staleness_ns: Option<i128>,
    altered_fields: Vec<String>,
    support_depth: crate::profile::support::SupportDepth,
    identifier_rules: IdentifierRules,
}

impl CapabilityDeclaration {
    /// On the declaration because the identifier is minted above the adapter and the rules live
    /// below it.
    pub fn identifier_rules(&self) -> &IdentifierRules {
        &self.identifier_rules
    }

    /// Asked of the declaration because an executor is built from it, so the check cannot be
    /// forgotten.
    pub fn permits_execution(&self) -> bool {
        matches!(self.support_depth, crate::profile::support::SupportDepth::Full)
    }
}

impl CapabilityDeclaration {
    pub fn from_profile(p: &VenueProfile) -> Self {
        let mut classes: Vec<DataClass> =
            p.transports.values().flat_map(|t| t.data_classes.iter().copied()).collect();
        classes.sort_unstable();
        classes.dedup();

        // Tightest bound across transports: an unnamed consumer gets the guarantee that holds
        // everywhere.
        let staleness = p.transports.values().filter_map(|t| t.max_staleness_ns).min();

        Self {
            venue: p.venue.clone(),
            verified: p.verified_at.is_some(),
            data_classes: classes,
            native_order_types: p
                .order_types
                .iter()
                .filter(|o| o.native)
                .map(|o| o.name.clone())
                .collect(),
            reduce_only: p.reduce_only,
            funding_by_transport: p.transports.iter().map(|(t, tp)| (*t, tp.funding)).collect(),
            instruments: p.instruments.clone(),
            max_staleness_ns: staleness,
            altered_fields: p.transformations.iter().map(|t| t.field().to_string()).collect(),
            support_depth: p.support.depth(),
            identifier_rules: p.identifier_rules.clone(),
        }
    }

    pub fn venue(&self) -> &str {
        &self.venue
    }

    /// No venue-wide equivalent: no granularity is true of every instrument.
    pub fn precision(&self, symbol: &VenueSymbol) -> Result<&PrecisionRules, CatalogueError> {
        self.instruments.precision(symbol)
    }

    pub fn instruments(&self) -> &InstrumentCatalogue {
        &self.instruments
    }

    pub fn is_verified(&self) -> bool {
        self.verified
    }

    /// Consumers adapt or refuse on this. An emulated type answers `false`.
    pub fn supports_natively(&self, order_type: &str) -> bool {
        self.native_order_types.iter().any(|n| n == order_type)
    }

    pub fn serves(&self, class: DataClass) -> bool {
        self.data_classes.contains(&class)
    }

    pub fn reduce_only(&self) -> ReduceOnly {
        self.reduce_only
    }

    pub fn funding(&self, transport: Transport) -> Option<FundingModel> {
        self.funding_by_transport.get(&transport).copied()
    }

    pub fn max_staleness_ns(&self) -> Option<i128> {
        self.max_staleness_ns
    }

    /// Fields the venue alters in flight. A benchmark over one of these is measuring a fiction.
    pub fn altered_fields(&self) -> &[String] {
        &self.altered_fields
    }

    /// Pricing on an altered field prices something other than what is sent. Desk admission
    /// refuses such an order.
    #[must_use]
    pub fn alters(&self, field: &str) -> bool {
        self.altered_fields.iter().any(|f| f == field)
    }
}

/// What the venue was seen doing, beside the declaration it is compared against. A field never
/// observed is an absence, not a divergence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedBehaviour {
    pub reduce_only: Option<ReduceOnly>,
    pub native_order_types: Option<Vec<String>>,
    pub funding: BTreeMap<Transport, FundingModel>,
    /// The symbol travels with the rules: a divergence finding must say which instrument it
    /// concerns.
    pub precision: Option<ObservedPrecision>,
    /// Added 2026-08-26: nothing checked V7's declared minimums against the venue, so a floor
    /// of nothing passed.
    pub minimums: Option<ObservedMinimums>,
    /// Added 2026-11-15: the currencies an instrument is counted, quoted and settled in, as the
    /// venue lists them.
    pub currencies: Option<ObservedCurrencies>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedCurrencies {
    pub symbol: VenueSymbol,
    pub base: String,
    /// `None` where the venue's answer does not state it: unobserved, never assumed.
    pub quote: Option<String>,
    pub settlement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedMinimums {
    pub symbol: VenueSymbol,
    /// In the instrument's quantity scale.
    pub minimum_size: i128,
    /// In the instrument's price scale.
    pub minimum_notional: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPrecision {
    pub symbol: VenueSymbol,
    pub rules: PrecisionRules,
}
