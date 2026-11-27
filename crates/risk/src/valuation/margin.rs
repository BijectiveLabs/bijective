// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Leverage and margin use for the whole book, from what each venue states backs its account.
//!
//! Leverage is the book's gross exposure over the venues' equity. Margin use is the venues'
//! maintenance margin over that equity, where one is the point a venue may liquidate. Both are
//! ratios at [`Ratio::ONE`], rounded up so a figure is never below the exact one. The equity alone
//! is also the book's equity over time, recorded whether or not a limit declares a ratio.

use bijective_limits::{Amount, ArithmeticError, LimitKind, Ratio, Scope, Threshold, notional};
use bijective_model::{AccountMargin, InstrumentId};

use super::Unvalued;
use super::currency::Conversion;

/// Why no margin figure was made. Without one a declared limit is unjudged, which halts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarginUnmeasured {
    /// A venue read this pass states no margin: the equity would be part of the account's.
    NotStated {
        venue: String,
    },
    /// Equity at or below zero: there is nothing a ratio could be taken over.
    NoEquity,
    Unconverted(Unvalued),
    Arithmetic(ArithmeticError),
}

/// The portfolio's leverage and margin use. `margins` holds every venue read this pass. `gross`
/// is the book's gross exposure in `book`'s currency.
pub fn margin_figures(
    margins: &[(String, Option<AccountMargin>)],
    gross: Amount,
    book: &mut Conversion<'_>,
) -> Result<[(Scope, LimitKind, Threshold); 2], MarginUnmeasured> {
    let mut equity = Amount::from_scaled(0);
    let mut maintenance = Amount::from_scaled(0);
    for (venue, margin) in margins {
        let margin =
            margin.as_ref().ok_or_else(|| MarginUnmeasured::NotStated { venue: venue.clone() })?;
        let stated = |scaled: i128| StatedEquity {
            venue: venue.clone(),
            currency: margin.currency.clone(),
            equity_scaled: scaled,
            scale: margin.scale,
        };
        let venue_equity = in_book(&stated(margin.equity_scaled), book)?;
        let venue_maintenance = in_book(&stated(margin.maintenance_scaled), book)?;
        equity = equity.add(venue_equity).map_err(MarginUnmeasured::Arithmetic)?;
        maintenance = maintenance.add(venue_maintenance).map_err(MarginUnmeasured::Arithmetic)?;
    }
    Ok([
        (Scope::Portfolio, LimitKind::Leverage, Threshold::Ratio(ratio(gross, equity)?)),
        (Scope::Portfolio, LimitKind::MarginUse, Threshold::Ratio(ratio(maintenance, equity)?)),
    ])
}

/// One venue's equity in one currency, as the venue states it for the account or as a sandbox
/// account holds it, at `scale` decimals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatedEquity {
    pub venue: String,
    pub currency: String,
    pub equity_scaled: i128,
    pub scale: u8,
}

/// The sum of `stated` in `book`'s currency. A figure that cannot be converted fails the whole:
/// an equity missing a venue would read as a loss.
pub fn book_equity(
    stated: &[StatedEquity],
    book: &mut Conversion<'_>,
) -> Result<Amount, MarginUnmeasured> {
    let mut equity = Amount::from_scaled(0);
    for part in stated {
        equity = equity.add(in_book(part, book)?).map_err(MarginUnmeasured::Arithmetic)?;
    }
    Ok(equity)
}

fn in_book(part: &StatedEquity, book: &mut Conversion<'_>) -> Result<Amount, MarginUnmeasured> {
    let on = InstrumentId::new(format!("{}.account", part.venue));
    let amount =
        notional(part.equity_scaled, part.scale, 1, 0).map_err(MarginUnmeasured::Arithmetic)?;
    book.into_book(amount, &part.currency, &on).map_err(MarginUnmeasured::Unconverted)
}

/// `part / whole` at [`Ratio::ONE`], rounded up.
fn ratio(part: Amount, whole: Amount) -> Result<Ratio, MarginUnmeasured> {
    if whole.scaled() <= 0 {
        return Err(MarginUnmeasured::NoEquity);
    }
    let scaled = part
        .scaled()
        .checked_mul(Ratio::ONE)
        .ok_or(MarginUnmeasured::Arithmetic(ArithmeticError::Overflow))?;
    let quotient = scaled / whole.scaled();
    let rounded = if scaled % whole.scaled() > 0 { quotient + 1 } else { quotient };
    Ok(Ratio::from_scaled(rounded))
}

impl core::fmt::Display for MarginUnmeasured {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotStated { venue } => write!(f, "{venue} states no account margin"),
            Self::NoEquity => write!(f, "the venues' equity is not above zero"),
            Self::Unconverted(why) => write!(f, "the equity could not be converted: {why:?}"),
            Self::Arithmetic(why) => write!(f, "{why:?}"),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/valuation/margin_tests.rs"]
mod tests;
