// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Adds each desk's and each strategy's exposure to a valuation, when a limit at either level is
//! declared.

use bijective_control_engine::OrderPath;
use bijective_limits::{LimitKind, Scope, ScopeLevel, Threshold};
use bijective_model::HeldPosition;
use bijective_risk::desks::MandateBook;
use bijective_risk::valuation::attribution::{Placed, attributed};
use bijective_risk::valuation::currency::Conversion;
use bijective_risk::valuation::{Pricing, Valuation};

use crate::PROGRAM;
use crate::venues::OpenVenues;

/// What a pass knows when the desk and strategy figures are made.
pub(crate) struct Scene<'a> {
    pub(crate) path: &'a OrderPath,
    pub(crate) limits: &'a [(Scope, LimitKind, Threshold)],
    pub(crate) mandates: &'a MandateBook,
    pub(crate) adapters: &'a OpenVenues,
    pub(crate) held: &'a [HeldPosition],
    pub(crate) pricing: Pricing<'a>,
}

/// Nothing is attached when the book could not be valued, when the order record could not be
/// rebuilt, or when an order or position cannot be priced: a declared desk or strategy limit is
/// then unjudged and the node halts, which is the intended failure. An exposure that cannot be
/// measured is not an exposure within bounds.
pub(crate) fn attach_attribution(scene: &Scene<'_>, valued: &mut Valuation) {
    let named: Vec<Scope> = scene
        .limits
        .iter()
        .map(|(scope, _, _)| scope)
        .filter(|s| matches!(s.level(), ScopeLevel::Desk | ScopeLevel::Strategy))
        .cloned()
        .collect();
    if named.is_empty() || valued.snapshot.figures.is_empty() {
        return;
    }
    let Some(orders) = scene.path.recorded_orders() else { return };
    let identity = |venue: &str, symbol: &bijective_model::VenueSymbol| {
        let (_, adapter) = scene.adapters.iter().find(|(name, _)| name == venue)?;
        adapter.declaration().instruments().identity_of(symbol).ok().cloned()
    };
    let placed = match orders.placed(identity) {
        Ok(placed) => {
            placed.into_iter().map(|p| on_a_declared_desk(p, scene.mandates)).collect::<Vec<_>>()
        }
        Err((venue, symbol)) => {
            eprintln!(
                "{PROGRAM}: desk and strategy exposure is unmeasured: {venue} does not catalogue \
                 {symbol}, which an order was placed on"
            );
            return;
        }
    };
    // In the currency the portfolio was valued in, so every level is measured in one.
    let mut book = Conversion::new(scene.pricing, valued.currency.as_deref());
    match attributed(scene.held, &placed, &mut book, &named) {
        Ok(measured) => {
            let oldest = valued.snapshot.observed_at_ns.min(measured.observed_at_ns);
            valued.snapshot.observed_at_ns = oldest;
            valued.snapshot.figures.extend(measured.figures);
        }
        Err(why) => eprintln!("{PROGRAM}: desk and strategy exposure is unmeasured: {why}"),
    }
}

/// An owner whose desk is not declared is read as no owner: an adjustment typed against a
/// misspelt desk would otherwise leave the desk it meant understated.
fn on_a_declared_desk(mut placed: Placed, mandates: &MandateBook) -> Placed {
    if placed.owner.as_ref().is_some_and(|o| mandates.get(&o.desk).is_none()) {
        placed.owner = None;
    }
    placed
}
