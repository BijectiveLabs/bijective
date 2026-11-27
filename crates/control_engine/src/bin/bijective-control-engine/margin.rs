// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Adds the portfolio's leverage and margin use to a valuation, when a leverage or margin-use limit
//! is declared, from the margin each venue read this pass states for its account, and makes the
//! book's equity from the same statements.

use bijective_limits::{LimitKind, Scope, Threshold};
use bijective_model::AccountMargin;
use bijective_risk::valuation::currency::Conversion;
use bijective_risk::valuation::margin::{StatedEquity, book_equity, margin_figures};
use bijective_risk::valuation::{Pricing, Valuation};

use crate::PROGRAM;

/// Without the book's gross figure (anything unvalued) nothing is attached. A figure that cannot
/// be measured leaves the declared limit unjudged, which halts: leverage that cannot be shown is
/// not leverage within bounds.
pub(crate) fn attach_margin(
    limits: &[(Scope, LimitKind, Threshold)],
    margins: &[(String, Option<AccountMargin>)],
    pricing: Pricing<'_>,
    valued: &mut Valuation,
) {
    let declared = limits
        .iter()
        .any(|(_, kind, _)| matches!(kind, LimitKind::Leverage | LimitKind::MarginUse));
    if !declared {
        return;
    }
    let gross = valued.snapshot.figures.iter().find_map(|(scope, kind, figure)| {
        match (scope, kind, figure) {
            (Scope::Portfolio, LimitKind::GrossExposure, Threshold::Money(amount)) => Some(*amount),
            _ => None,
        }
    });
    let Some(gross) = gross else { return };
    let mut book = Conversion::new(pricing, valued.currency.as_deref());
    match margin_figures(margins, gross, &mut book) {
        Ok(figures) => valued.snapshot.figures.extend(figures),
        Err(why) => eprintln!("{PROGRAM}: leverage and margin use are unmeasured: {why}"),
    }
}

/// What each venue states backs its account this pass: a live venue's stated margin equity, or a
/// simulated venue's cash with its positions at the node's marks. `None` when a venue is
/// withdrawn or states none, since an equity missing a venue would read as a loss.
pub(crate) fn stated_equity(
    margins: &[(String, Option<AccountMargin>)],
    withdrawn: &[String],
    paper: &crate::paper::Paper,
    pricing: Pricing<'_>,
) -> Option<Vec<StatedEquity>> {
    if !withdrawn.is_empty() {
        return None;
    }
    if paper.is_simulated() {
        return paper.stated_equity(pricing.marks);
    }
    margins
        .iter()
        .map(|(venue, margin)| {
            let margin = margin.as_ref()?;
            Some(StatedEquity {
                venue: venue.clone(),
                currency: margin.currency.clone(),
                equity_scaled: margin.equity_scaled,
                scale: margin.scale,
            })
        })
        .collect()
}

/// `stated` in `valued`'s book currency, at the money scale. `None` when a part cannot be
/// converted. Not reported each pass: the absent figure in the record says it.
pub(crate) fn book_equity_of(
    stated: &[StatedEquity],
    pricing: Pricing<'_>,
    valued: &Valuation,
) -> Option<i128> {
    let mut book = Conversion::new(pricing, valued.currency.as_deref());
    book_equity(stated, &mut book).ok().map(|equity| equity.scaled())
}
