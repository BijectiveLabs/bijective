// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The portfolio's loss so far today: what was realised, what fees were paid, what is open.
//! The result of a day is what its fills realised, less the fees they paid, plus how far the
//! unrealised profit moved since the day opened. A fee paid in an asset the book can be valued in
//! is counted. One unstated, credited, or in an asset with no rate is left out and counted apart,
//! so the operator sees by how many fees the figure falls short. Precondition: the caller supplies
//! a baseline only for the day being measured. Postcondition: a figure only when every realised
//! part was priced, in the book's currency (`docs/decisions/0005-reporting-currency.md`).

use std::collections::BTreeMap;

use bijective_limits::{Amount, ArithmeticError, LimitKind, Scope, Threshold, notional};
use bijective_model::InstrumentId;
use bijective_risk::valuation::currency::Conversion;
use bijective_risk::valuation::{Unvalued, daily_loss, realized_value};
use bijective_venue::profile::Instrument;

use bijective_lifecycle::Fee;

use crate::orders::{AccountCharged, Charged, Realized};

/// Why no loss figure exists. The limit is then unjudged, and an unjudged limit halts the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmeasured {
    /// The day has not opened: no valued book has been seen today.
    NoBaseline,
    /// A realised result on an instrument the venue declarations do not carry.
    UnknownInstrument {
        venue: String,
        instrument: String,
    },
    Arithmetic(ArithmeticError),
    /// A realised result or fee in a currency that could not be brought into the book's.
    Unconverted(Unvalued),
}

impl std::fmt::Display for Unmeasured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBaseline => write!(f, "the day has no opening baseline"),
            Self::UnknownInstrument { venue, instrument } => {
                write!(f, "a fill on {instrument} at {venue} cannot be priced")
            }
            Self::Arithmetic(cause) => write!(f, "the loss could not be computed: {cause:?}"),
            Self::Unconverted(why) => write!(f, "the loss could not be converted: {why}"),
        }
    }
}

/// The loss, and how many of today's fees it could not include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossFigure {
    pub figure: (Scope, LimitKind, Threshold),
    /// Fees unstated, stated in another asset, or a rebate. A rebate is left out until the sign
    /// of a venue's rebate figure is confirmed: crediting it wrongly would understate the loss.
    pub fees_not_valued: u64,
}

fn instrument_of<'a>(
    instruments: &'a BTreeMap<(String, String), Instrument>,
    venue: &str,
    symbol: &str,
) -> Result<&'a Instrument, Unmeasured> {
    instruments.get(&(venue.to_string(), symbol.to_string())).ok_or_else(|| {
        Unmeasured::UnknownInstrument { venue: venue.to_string(), instrument: symbol.to_string() }
    })
}

/// What the day's records hold: what its fills realised and what they were charged.
#[derive(Debug, Clone, Copy)]
pub struct Today<'a> {
    pub realized: &'a [Realized],
    pub charged: &'a [Charged],
    /// Charges to venue accounts that name no order, such as a daily regulatory fee.
    pub account: &'a [AccountCharged],
}

/// The loss figure at portfolio scope, in `book`'s currency. `instruments` is keyed by venue and
/// venue symbol. `baseline` and `unrealized_now` are already in the book's currency.
pub fn daily_loss_figure(
    baseline: Option<Amount>,
    unrealized_now: Amount,
    today: Today<'_>,
    instruments: &BTreeMap<(String, String), Instrument>,
    book: &mut Conversion<'_>,
) -> Result<LossFigure, Unmeasured> {
    let baseline = baseline.ok_or(Unmeasured::NoBaseline)?;
    let mut result = Amount::from_scaled(0);
    for fill in today.realized {
        let instrument = instrument_of(instruments, &fill.venue, fill.instrument.as_str())?;
        let value =
            realized_value(fill.realized_scaled, instrument).map_err(Unmeasured::Arithmetic)?;
        let value = book
            .into_book(value, &instrument.settlement_currency, &instrument.id)
            .map_err(Unmeasured::Unconverted)?;
        result = result.add(value).map_err(Unmeasured::Arithmetic)?;
    }
    let mut fees_not_valued = 0;
    for charged in today.charged {
        let instrument = instrument_of(instruments, &charged.venue, charged.instrument.as_str())?;
        match paid_in_book(charged.fee.as_ref(), &instrument.id, book)? {
            Some(value) => result = result.sub(value).map_err(Unmeasured::Arithmetic)?,
            None => fees_not_valued += 1,
        }
    }
    for charged in today.account {
        let on = InstrumentId::new(charged.venue.clone());
        match paid_in_book(Some(&charged.fee), &on, book)? {
            Some(value) => result = result.sub(value).map_err(Unmeasured::Arithmetic)?,
            None => fees_not_valued += 1,
        }
    }
    let figure = daily_loss(result, unrealized_now, baseline).map_err(Unmeasured::Arithmetic)?;
    Ok(LossFigure { figure, fees_not_valued })
}

/// A fee paid, in the book's currency. `None` when it is unstated, credited (a rebate's sign is
/// not confirmed on every venue), or in an asset with no rate into the book's currency. The asset
/// is compared without regard to letter case, as a venue writes `USDT` and a profile `usdt`.
fn paid_in_book(
    fee: Option<&Fee>,
    on: &InstrumentId,
    book: &mut Conversion<'_>,
) -> Result<Option<Amount>, Unmeasured> {
    let Some(fee) = fee.filter(|f| f.amount_scaled > 0 && !f.asset.is_empty()) else {
        return Ok(None);
    };
    let value = notional(fee.amount_scaled, fee.scale, 1, 0).map_err(Unmeasured::Arithmetic)?;
    match book.into_book(value, &fee.asset, on) {
        Ok(value) => Ok(Some(value)),
        Err(Unvalued::ForeignSettlement { .. } | Unvalued::NoRate { .. }) => Ok(None),
        Err(why) => Err(Unmeasured::Unconverted(why)),
    }
}

/// The day's result: the loss turned over. Kept as one definition so the loss and the drawdown
/// cannot measure different days.
#[must_use]
pub fn day_result(loss: &LossFigure) -> Option<Amount> {
    match loss.figure.2 {
        Threshold::Money(loss) => Amount::from_scaled(0).sub(loss).ok(),
        Threshold::Ratio(_) => None,
    }
}

/// How far `result` stands below the day's highest result, at portfolio scope. The day opens at
/// a result of zero, so the highest point is never below it. `peak` is the highest recorded.
pub fn drawdown_figure(
    result: Amount,
    peak: Option<Amount>,
) -> Result<(Scope, LimitKind, Threshold), Unmeasured> {
    let open = Amount::from_scaled(0);
    let high = open.max(peak.unwrap_or(open)).max(result);
    let fallen = high.sub(result).map_err(Unmeasured::Arithmetic)?;
    Ok((Scope::Portfolio, LimitKind::Drawdown, Threshold::Money(fallen)))
}

#[cfg(test)]
#[path = "../tests/unit/loss_tests.rs"]
mod tests;
