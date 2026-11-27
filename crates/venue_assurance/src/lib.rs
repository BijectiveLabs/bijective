// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Checks that a venue still behaves as its profile says, and never corrects it.

pub use bijective_venue::profile::ObservedBehaviour;

pub mod published;
pub mod wire;

use bijective_venue::profile::{
    FundingModel, PrecisionRules, ReduceOnly, Transport, VenueProfile, VenueSymbol,
};

/// One way the venue and its profile disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    ReduceOnly {
        declared: ReduceOnly,
        observed: ReduceOnly,
    },
    /// The profile claims native support the venue lacks. Risk limits may be judging against a guarantee never made.
    OrderTypeNotNative {
        name: String,
    },
    /// Understating is safe for capital but wrong: a consumer refuses work it could do.
    OrderTypeNativeUndeclared {
        name: String,
    },
    Funding {
        transport: Transport,
        declared: FundingModel,
        observed: FundingModel,
    },
    Precision {
        symbol: VenueSymbol,
        declared: PrecisionRules,
        observed: PrecisionRules,
    },
    /// The venue enforces a floor the profile does not declare. Capital-relevant: an order admitted on the lower declared floor is refused at placement.
    Minimums {
        symbol: VenueSymbol,
        declared_size: i128,
        observed_size: i128,
        declared_notional: i128,
        observed_notional: i128,
    },
    /// An instrument the catalogue does not carry. Capital-relevant: nothing to validate against.
    InstrumentUndeclared {
        symbol: VenueSymbol,
    },
    /// The profile counts, quotes or settles an instrument in another currency than the venue,
    /// or does not declare what it is counted in. Capital-relevant: exposure is valued and summed
    /// by these currencies.
    Currencies {
        symbol: VenueSymbol,
        declared: [Option<String>; 3],
        observed: [Option<String>; 3],
    },
}

impl Divergence {
    /// Whether this one can cause the platform to act on a guarantee the venue did not make.
    pub fn is_capital_relevant(&self) -> bool {
        matches!(
            self,
            Divergence::ReduceOnly { .. }
                | Divergence::OrderTypeNotNative { .. }
                | Divergence::Precision { .. }
                | Divergence::InstrumentUndeclared { .. }
                | Divergence::Minimums { .. }
                | Divergence::Currencies { .. }
        )
    }
}

/// The outcome of one check. No variant or method yields a corrected profile: the discrepancy is the finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    Confirmed {
        at_ns: i128,
    },
    Diverged {
        at_ns: i128,
        findings: Vec<Divergence>,
    },
    /// The venue could not be observed. Distinct from `Confirmed`: no answer is not a confirmation.
    NotObserved {
        at_ns: i128,
        reason: NotObservedReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotObservedReason {
    VenueUnreachable,
    /// The profile declares a retrieval method the venue no longer serves: a profile defect, routed to venue assurance.
    RetrievalMethodGone,
}

impl Verification {
    /// Whether trading may rely on this venue's declaration.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Verification::Confirmed { .. })
    }

    /// When the profile was last *checked*, whatever the answer.
    pub fn at_ns(&self) -> i128 {
        match self {
            Verification::Confirmed { at_ns }
            | Verification::Diverged { at_ns, .. }
            | Verification::NotObserved { at_ns, .. } => *at_ns,
        }
    }
}

/// Compare a profile against observed behaviour. Shared reference by design: a check cannot rewrite what it checks.
pub fn verify(profile: &VenueProfile, observed: &ObservedBehaviour, at_ns: i128) -> Verification {
    let mut findings = Vec::new();
    compare_reduce_only(profile, observed, &mut findings);
    compare_order_types(profile, observed, &mut findings);
    compare_funding(profile, observed, &mut findings);
    compare_precision(profile, observed, &mut findings);
    compare_minimums(profile, observed, &mut findings);
    compare_currencies(profile, observed, &mut findings);
    if findings.is_empty() {
        Verification::Confirmed { at_ns }
    } else {
        Verification::Diverged { at_ns, findings }
    }
}

fn compare_reduce_only(
    profile: &VenueProfile,
    observed: &ObservedBehaviour,
    findings: &mut Vec<Divergence>,
) {
    if let Some(seen) = observed.reduce_only
        && seen != profile.reduce_only
    {
        findings.push(Divergence::ReduceOnly { declared: profile.reduce_only, observed: seen });
    }
}

/// Both directions: a declared native type the venue lacks, and a native type the profile omits.
fn compare_order_types(
    profile: &VenueProfile,
    observed: &ObservedBehaviour,
    findings: &mut Vec<Divergence>,
) {
    if let Some(seen) = &observed.native_order_types {
        for declared in profile.order_types.iter().filter(|o| o.native) {
            if !seen.contains(&declared.name) {
                findings.push(Divergence::OrderTypeNotNative { name: declared.name.clone() });
            }
        }
        for name in seen {
            let claimed = profile.order_types.iter().any(|o| o.native && &o.name == name);
            if !claimed {
                findings.push(Divergence::OrderTypeNativeUndeclared { name: name.clone() });
            }
        }
    }
}

fn compare_funding(
    profile: &VenueProfile,
    observed: &ObservedBehaviour,
    findings: &mut Vec<Divergence>,
) {
    for (transport, seen) in &observed.funding {
        if let Some(tp) = profile.transports.get(transport)
            && tp.funding != *seen
        {
            findings.push(Divergence::Funding {
                transport: *transport,
                declared: tp.funding,
                observed: *seen,
            });
        }
    }
}

fn compare_precision(
    profile: &VenueProfile,
    observed: &ObservedBehaviour,
    findings: &mut Vec<Divergence>,
) {
    if let Some(seen) = &observed.precision {
        match profile.instruments.precision(&seen.symbol) {
            Ok(declared) if !declared.states_the_same_rule(&seen.rules) => {
                findings.push(Divergence::Precision {
                    symbol: seen.symbol.clone(),
                    declared: declared.clone(),
                    observed: seen.rules.clone(),
                })
            }
            Ok(_) => {}
            Err(_) => {
                findings.push(Divergence::InstrumentUndeclared { symbol: seen.symbol.clone() })
            }
        }
    }
}

fn compare_minimums(
    profile: &VenueProfile,
    observed: &ObservedBehaviour,
    findings: &mut Vec<Divergence>,
) {
    if let Some(seen) = &observed.minimums {
        match profile.instruments.get(&seen.symbol) {
            Ok(declared)
                if declared.minimum_size != seen.minimum_size
                    || declared.minimum_notional != seen.minimum_notional =>
            {
                findings.push(Divergence::Minimums {
                    symbol: seen.symbol.clone(),
                    declared_size: declared.minimum_size,
                    observed_size: seen.minimum_size,
                    declared_notional: declared.minimum_notional,
                    observed_notional: seen.minimum_notional,
                });
            }
            Ok(_) => {}
            Err(_) => {
                findings.push(Divergence::InstrumentUndeclared { symbol: seen.symbol.clone() })
            }
        }
    }
}

/// Base, quote and settlement, compared without regard to letter case: a venue writes `USDT`
/// where a profile may write `usdt`, and they name one currency. An undeclared base diverges.
fn compare_currencies(
    profile: &VenueProfile,
    observed: &ObservedBehaviour,
    findings: &mut Vec<Divergence>,
) {
    let Some(seen) = &observed.currencies else { return };
    let Ok(declared) = profile.instruments.get(&seen.symbol) else {
        findings.push(Divergence::InstrumentUndeclared { symbol: seen.symbol.clone() });
        return;
    };
    let same = |a: Option<&str>, b: &str| a.is_some_and(|a| a.eq_ignore_ascii_case(b));
    // A currency the venue does not state is not compared: no answer is not a disagreement.
    let agrees = |a: &str, b: &Option<String>| b.as_deref().is_none_or(|b| same(Some(a), b));
    if same(declared.base_currency.as_deref(), &seen.base)
        && agrees(&declared.quote_currency, &seen.quote)
        && agrees(&declared.settlement_currency, &seen.settlement)
    {
        return;
    }
    findings.push(Divergence::Currencies {
        symbol: seen.symbol.clone(),
        declared: [
            declared.base_currency.clone(),
            Some(declared.quote_currency.clone()),
            Some(declared.settlement_currency.clone()),
        ],
        observed: [Some(seen.base.clone()), seen.quote.clone(), seen.settlement.clone()],
    });
}

/// What a check produces. The caller persists it, as with the pre-trade check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssuranceReport {
    pub venue: String,
    pub verification: Verification,
}

impl AssuranceReport {
    pub fn new(venue: &str, verification: Verification) -> Self {
        Self { venue: venue.to_string(), verification }
    }

    /// Whether missing-event detection should raise a condition. A failed observation raises one too: a check that could not run did not pass.
    pub fn raises_condition(&self) -> bool {
        !self.verification.is_confirmed()
    }

    /// Marked unverified as of this instant, beside the profile and never written into it.
    pub fn unverified_as_of(&self) -> Option<i128> {
        if self.verification.is_confirmed() { None } else { Some(self.verification.at_ns()) }
    }
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

impl core::fmt::Display for Divergence {
    /// Declared and observed sides are both printed: an operator needs the half that has to change.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Divergence::ReduceOnly { declared, observed } => {
                write!(f, "reduce-only: declared {declared:?}, observed {observed:?}")
            }
            Divergence::OrderTypeNotNative { name } => {
                write!(f, "{name} is claimed as native and the venue does not provide it")
            }
            Divergence::OrderTypeNativeUndeclared { name } => {
                write!(f, "{name} is native at the venue and is not claimed")
            }
            Divergence::Funding { transport, declared, observed } => {
                write!(f, "funding on {transport:?}: declared {declared:?}, observed {observed:?}")
            }
            Divergence::Precision { symbol, declared, observed } => write!(
                f,
                "precision on {}: declared {declared:?}, observed {observed:?}",
                symbol.as_str()
            ),
            Divergence::Minimums {
                symbol,
                declared_size,
                observed_size,
                declared_notional,
                observed_notional,
            } => write!(
                f,
                "minimums on {}: size declared {declared_size} and observed {observed_size}, \
                 notional declared {declared_notional} and observed {observed_notional}",
                symbol.as_str()
            ),
            Divergence::InstrumentUndeclared { symbol } => {
                write!(f, "{} is traded at the venue and is not declared", symbol.as_str())
            }
            Divergence::Currencies { symbol, declared, observed } => write!(
                f,
                "currencies (base, quote, settlement) on {}: declared {declared:?}, observed \
                 {observed:?}",
                symbol.as_str()
            ),
        }
    }
}
