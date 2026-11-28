// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Turns a desk proposal into an admitted order or a named refusal.

use std::collections::{BTreeMap, BTreeSet};

use bijective_limits::{Amount, ArithmeticError, Scope, notional};
use bijective_model::{InstrumentId, OrderRequest, Proposal, VenueSymbol};
use bijective_venue::profile::{
    CapabilityDeclaration, IdentifierRejection, IncrementRejection, Instrument, InstrumentKind,
    PrecisionRules,
};

/// What a desk is permitted to do, as the desk declared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mandate {
    /// Keyed by platform identity, not venue symbol, so the mandate binds on every venue.
    pub instruments: BTreeSet<InstrumentId>,
    pub venue_preference: Vec<String>,
    pub maximum_per_order: Amount,
    /// How far, in basis points of the mark, a limit price may cross it (`collar`).
    pub price_collar_bps: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MandateBook {
    by_desk: BTreeMap<String, Mandate>,
}

impl MandateBook {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(&mut self, desk: impl Into<String>, mandate: Mandate) {
        self.by_desk.insert(desk.into(), mandate);
    }

    #[must_use]
    pub fn get(&self, desk: &str) -> Option<&Mandate> {
        self.by_desk.get(desk)
    }
}

/// Why a proposal did not become a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    UnknownDesk {
        desk: String,
    },
    NotMandated {
        instrument: InstrumentId,
    },
    NoCapDeclared,
    /// Post-only needs a limit price to rest at, and a time in force that lets it rest.
    PostOnlyCannotRest,
    /// A stop is held until it triggers and then takes. Post-only would cancel what it is for.
    StopCannotBePostOnly,
    NonPositiveStop,
    /// An expiry (decision 0014) cancels an order that rests. One that cannot rest has none.
    ExpiryCannotRest,
    NonPositiveQuantity,
    /// Refused, not resized: a resized order would be attributed to a strategy that never proposed it.
    AboveMaximum {
        requested: Amount,
        maximum: Amount,
    },
    /// The proposal named a venue the desk does not route to. Refused, never redirected: a decision
    /// made for one venue's price, fee or liquidity is not the same decision on another.
    VenueNotMandated {
        venue: String,
    },
    /// Never falls back to a venue the desk did not prefer.
    NoEligibleVenue {
        instrument: InstrumentId,
    },
    /// A preferred venue lists the instrument as a kind the platform does not trade (a dated
    /// future or an inverse contract). Distinct from `NoEligibleVenue`, which says nothing about why.
    KindNotTradable {
        instrument: InstrumentId,
        kind: InstrumentKind,
    },
    /// Checked before placement. The increment travels with the refusal.
    PriceOffIncrement {
        value: i128,
        increment: i128,
    },
    QuantityOffIncrement {
        value: i128,
        increment: i128,
    },
    /// The identifier drives duplicate detection after an unknown outcome, so it must be one the venue keeps intact.
    IdentifierRefused {
        rejection: IdentifierRejection,
    },
    /// An integer price is always accepted by the venue. The check must keep that exception.
    PriceTooPrecise {
        value: i128,
        at_most: u8,
    },
    /// Both figures at platform scale.
    BelowMinimumNotional {
        spend: Amount,
        minimum: Amount,
    },
    /// An outcome share priced at or beyond its payout bounds, `0` and one unit of collateral, at
    /// the instrument's price scale.
    OutsideOutcomeBounds {
        price: i128,
        payout: i128,
    },
    /// The reservation is priced from a field the venue would change in flight.
    WouldBeAltered {
        field: String,
    },
    /// A limit price crossing the mark by more than the desk's collar, both at the instrument's
    /// price scale.
    OutsidePriceCollar {
        price: i128,
        mark: i128,
        collar_bps: u32,
    },
    /// A stop the market has already passed: a buy stop at or below the mark, a sell stop at or
    /// above it. It would trigger at once, or the venue would refuse it.
    StopAlreadyCrossed {
        stop: i128,
        mark: i128,
    },
    /// No mark to collar a limit price against, so the price cannot be judged.
    NoMarkForCollar {
        instrument: InstrumentId,
    },
    MarkTooOldForCollar {
        age_ns: i128,
        max_age_ns: i128,
    },
    Arithmetic(ArithmeticError),
}

/// A proposal, admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    pub venue: String,
    pub request: OrderRequest,
    pub spend: Amount,
    /// All-or-nothing draw.
    pub scopes: Vec<Scope>,
}

fn eligible<'a>(
    declaration: &'a CapabilityDeclaration,
    instrument: &InstrumentId,
) -> Option<&'a VenueSymbol> {
    declaration.instruments().symbols_for(instrument).into_iter().find(|symbol| {
        declaration
            .instruments()
            .get(symbol)
            .is_ok_and(|i| i.status.admits_orders() && i.kind.is_tradable())
    })
}

/// The first kind the platform does not trade that a preferred venue lists the instrument as, so
/// the refusal names the cause instead of reporting that no venue carries the instrument.
fn unsupported_kind(
    mandate: &Mandate,
    venues: &BTreeMap<String, CapabilityDeclaration>,
    instrument: &InstrumentId,
) -> Option<InstrumentKind> {
    mandate.venue_preference.iter().find_map(|name| {
        let declaration = venues.get(name)?;
        declaration.instruments().symbols_for(instrument).into_iter().find_map(|symbol| {
            let listed = declaration.instruments().get(symbol).ok()?;
            (listed.status.admits_orders() && !listed.kind.is_tradable()).then_some(listed.kind)
        })
    })
}

/// Whether the order terms make sense together: post-only rests, a stop takes, an expiry rests.
fn terms_hold_together(proposal: &Proposal) -> Result<(), Refusal> {
    let terms = proposal.terms;
    if terms.post_only && (proposal.price_scaled.is_none() || !terms.time_in_force.may_rest()) {
        return Err(Refusal::PostOnlyCannotRest);
    }
    match terms.stop_price_scaled {
        Some(_) if terms.post_only => return Err(Refusal::StopCannotBePostOnly),
        Some(stop) if stop <= 0 => return Err(Refusal::NonPositiveStop),
        _ => {}
    }
    if terms.expire_at_ns.is_some() && !terms.time_in_force.may_rest() {
        return Err(Refusal::ExpiryCannotRest);
    }
    Ok(())
}

pub fn admit(
    mandates: &MandateBook,
    venues: &BTreeMap<String, CapabilityDeclaration>,
    proposal: &Proposal,
    client_order_id: &str,
) -> Result<Admitted, Refusal> {
    let mandate = mandates
        .get(&proposal.desk)
        .ok_or_else(|| Refusal::UnknownDesk { desk: proposal.desk.clone() })?;

    if !mandate.instruments.contains(&proposal.instrument) {
        return Err(Refusal::NotMandated { instrument: proposal.instrument.clone() });
    }

    if proposal.quantity_scaled <= 0 {
        return Err(Refusal::NonPositiveQuantity);
    }
    terms_hold_together(proposal)?;

    let price = proposal.priced_at().ok_or(Refusal::NoCapDeclared)?;

    let Chosen { venue, symbol, declaration, instrument, precision } =
        choose_venue(mandate, venues, proposal)?;
    check_increments(declaration, precision, proposal)?;
    within_outcome_bounds(&instrument, precision, proposal)?;
    let spend = spend_above_minimum(
        precision,
        instrument.minimum_notional,
        proposal.quantity_scaled,
        price,
    )?;

    // Needs the chosen venue's rules, so it runs after venue choice and before the request is built.
    if let Err(rejection) = declaration.identifier_rules().check(client_order_id) {
        return Err(Refusal::IdentifierRefused { rejection });
    }

    if spend > mandate.maximum_per_order {
        return Err(Refusal::AboveMaximum { requested: spend, maximum: mandate.maximum_per_order });
    }

    let mut scopes = vec![
        Scope::Portfolio,
        Scope::Desk(proposal.desk.clone()),
        Scope::Venue(venue.clone()),
        Scope::Instrument(proposal.instrument.as_str().to_string()),
    ];
    // Bound only where the instrument declares its base. Without one no underlying figure exists
    // either, so a declared underlying limit is unjudged and nothing is placed.
    if let Some(asset) = crate::valuation::underlying_of(&instrument) {
        scopes.push(Scope::Underlying(asset));
    }
    // Bound only where the proposal names one: an unnamed strategy is no budget to draw on.
    if let Some(strategy) = &proposal.strategy {
        scopes.push(Scope::Strategy(strategy.clone()));
    }

    Ok(Admitted {
        venue,
        request: OrderRequest {
            client_id: client_order_id.to_string(),
            instrument: symbol,
            side: proposal.side,
            quantity_scaled: proposal.quantity_scaled,
            price_scaled: proposal.price_scaled,
            reduce_only: proposal.reduce_only,
            terms: proposal.terms,
        },
        spend,
        scopes,
    })
}

/// Among the desk's accounts in its own order, those the proposal named (an account by its name,
/// or every account at a venue by the venue's name, decision 0008), else all of them: the first
/// where the instrument is eligible, with its symbol, catalogue entry and precision rules. A named
/// account or venue where the instrument is not eligible is refused, not replaced by another.
fn choose_venue<'v>(
    mandate: &Mandate,
    venues: &'v BTreeMap<String, CapabilityDeclaration>,
    proposal: &Proposal,
) -> Result<Chosen<'v>, Refusal> {
    let not_eligible = || Refusal::NoEligibleVenue { instrument: proposal.instrument.clone() };
    let named_here = |account: &String, named: &String| {
        account == named || venues.get(account).is_some_and(|d| d.venue() == named.as_str())
    };
    let candidates: Vec<&String> = match &proposal.venue {
        Some(named) => {
            let at: Vec<&String> =
                mandate.venue_preference.iter().filter(|a| named_here(a, named)).collect();
            if at.is_empty() {
                return Err(Refusal::VenueNotMandated { venue: named.clone() });
            }
            at
        }
        None => mandate.venue_preference.iter().collect(),
    };
    let (venue, symbol, declaration) = candidates
        .into_iter()
        .find_map(|name| {
            let declaration = venues.get(name)?;
            let symbol = eligible(declaration, &proposal.instrument)?;
            Some((name.clone(), symbol.clone(), declaration))
        })
        .ok_or_else(|| {
            unsupported_kind(mandate, venues, &proposal.instrument).map_or_else(
                not_eligible,
                |kind| Refusal::KindNotTradable { instrument: proposal.instrument.clone(), kind },
            )
        })?;
    let instrument = declaration.instruments().get(&symbol).map_err(|_| not_eligible())?.clone();
    let precision = declaration.precision(&symbol).map_err(|_| not_eligible())?;
    Ok(Chosen { venue, symbol, declaration, instrument, precision })
}

/// The venue an order goes to and the rules it is checked against there.
struct Chosen<'v> {
    venue: String,
    symbol: VenueSymbol,
    declaration: &'v CapabilityDeclaration,
    instrument: Instrument,
    precision: &'v PrecisionRules,
}

/// Refuses a price or quantity off the venue's increments before the order leaves. A venue that
/// alters the price in flight is refused: the spend was priced on the price sent.
fn check_increments(
    declaration: &CapabilityDeclaration,
    precision: &PrecisionRules,
    proposal: &Proposal,
) -> Result<(), Refusal> {
    if declaration.alters("price") {
        return Err(Refusal::WouldBeAltered { field: "price".to_string() });
    }

    // A market order has no price to check. Its cap never reaches the venue. A stop does.
    let prices = [proposal.price_scaled, proposal.terms.stop_price_scaled];
    if let Some(cause) = prices.into_iter().flatten().find_map(|p| precision.check_price(p).err()) {
        return Err(match cause {
            IncrementRejection::OffIncrement { value, increment } => {
                Refusal::PriceOffIncrement { value, increment }
            }
            IncrementRejection::TooManySignificantFigures { value, at_most } => {
                Refusal::PriceTooPrecise { value, at_most }
            }
            IncrementRejection::IncrementNotDeclared => {
                Refusal::NoEligibleVenue { instrument: proposal.instrument.clone() }
            }
        });
    }

    if let Err(cause) = precision.check_quantity(proposal.quantity_scaled) {
        return Err(match cause {
            IncrementRejection::OffIncrement { value, increment } => {
                Refusal::QuantityOffIncrement { value, increment }
            }
            // No venue in scope has a significant-figure rule on sizes.
            IncrementRejection::TooManySignificantFigures { value, .. } => {
                Refusal::QuantityOffIncrement { value, increment: precision.quantity_lot }
            }
            IncrementRejection::IncrementNotDeclared => {
                Refusal::NoEligibleVenue { instrument: proposal.instrument.clone() }
            }
        });
    }
    Ok(())
}

/// A limit or a cap on an outcome share must lie strictly between nothing and the payout: no share
/// trades at or beyond what it can pay.
fn within_outcome_bounds(
    instrument: &Instrument,
    precision: &PrecisionRules,
    proposal: &Proposal,
) -> Result<(), Refusal> {
    if instrument.kind != InstrumentKind::Outcome {
        return Ok(());
    }
    let payout = 10i128.checked_pow(u32::from(precision.price_scale)).unwrap_or(i128::MAX);
    let stop = proposal.terms.stop_price_scaled;
    for price in [proposal.price_scaled, proposal.cap_scaled, stop].into_iter().flatten() {
        if price <= 0 || price >= payout {
            return Err(Refusal::OutsideOutcomeBounds { price, payout });
        }
    }
    Ok(())
}

/// The order's spend at the platform scale, refused below the instrument's minimum notional.
fn spend_above_minimum(
    precision: &PrecisionRules,
    minimum_notional: i128,
    quantity_scaled: i128,
    price: i128,
) -> Result<Amount, Refusal> {
    let spend = notional(quantity_scaled, precision.quantity_scale, price, precision.price_scale)
        .map_err(Refusal::Arithmetic)?;

    // Declared floor is in price scale. Convert before comparing.
    let floor =
        notional(minimum_notional, precision.price_scale, 1, 0).map_err(Refusal::Arithmetic)?;
    if spend < floor {
        return Err(Refusal::BelowMinimumNotional { spend, minimum: floor });
    }
    Ok(spend)
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refusal::UnknownDesk { desk } => write!(f, "no desk named {desk}"),
            Refusal::NotMandated { instrument } => {
                write!(f, "the desk has no mandate for {instrument}")
            }
            Refusal::NoCapDeclared => write!(f, "the desk declares no per-order cap"),
            Refusal::PostOnlyCannotRest => write!(
                f,
                "a post-only order needs a limit price and a time in force that lets it rest"
            ),
            Refusal::NonPositiveQuantity => write!(f, "the quantity is not positive"),
            Refusal::StopCannotBePostOnly => write!(f, "a stop order cannot be post-only"),
            Refusal::NonPositiveStop => write!(f, "the stop price is not positive"),
            Refusal::ExpiryCannotRest => {
                write!(f, "an expiry needs a time in force that lets the order rest")
            }
            Refusal::AboveMaximum { requested, maximum } => {
                write!(f, "{requested} was asked for against a cap of {maximum}")
            }
            Refusal::VenueNotMandated { venue } => {
                write!(f, "the proposal named {venue}, which this desk does not route to")
            }
            Refusal::NoEligibleVenue { instrument } => {
                write!(f, "no venue this desk may use carries {instrument}")
            }
            Refusal::KindNotTradable { instrument, kind } => {
                write!(f, "{instrument} is listed as {kind:?}; only spot and perpetual are traded")
            }
            Refusal::PriceOffIncrement { value, increment } => {
                write!(f, "the price {value} is not a multiple of {increment}")
            }
            Refusal::QuantityOffIncrement { value, increment } => {
                write!(f, "the quantity {value} is not a multiple of {increment}")
            }
            Refusal::IdentifierRefused { rejection } => {
                write!(f, "the identifier was refused: {rejection:?}")
            }
            Refusal::PriceTooPrecise { value, at_most } => {
                write!(f, "the price {value} carries more than {at_most} decimal place(s)")
            }
            Refusal::BelowMinimumNotional { spend, minimum } => {
                write!(f, "{spend} is below the venue's floor of {minimum}")
            }
            Refusal::OutsideOutcomeBounds { price, payout } => {
                write!(f, "the price {price} is not between 0 and the payout {payout}")
            }
            Refusal::WouldBeAltered { field } => {
                write!(f, "the venue would silently alter {field}")
            }
            Refusal::OutsidePriceCollar { price, mark, collar_bps } => write!(
                f,
                "the limit price {price} crosses the mark {mark} by more than {collar_bps} bps"
            ),
            Refusal::StopAlreadyCrossed { stop, mark } => {
                write!(f, "the stop {stop} is already crossed by the mark {mark}")
            }
            Refusal::NoMarkForCollar { instrument } => {
                write!(f, "no mark for {instrument} to judge the order against")
            }
            Refusal::MarkTooOldForCollar { age_ns, max_age_ns } => {
                write!(f, "the mark is {age_ns} ns old, beyond the {max_age_ns} ns bound")
            }
            Refusal::Arithmetic(error) => write!(f, "{error:?}"),
        }
    }
}
