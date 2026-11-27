// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Adds the portfolio's loss so far today, and how far today's result has fallen from its high,
//! to a valuation, when a daily-loss or drawdown limit is declared.

use std::collections::BTreeMap;

use bijective_control_engine::OrderPath;
use bijective_control_engine::loss::{Today, daily_loss_figure, day_result, drawdown_figure};
use bijective_limits::{Amount, LimitKind, Scope, Threshold};
use bijective_risk::valuation::currency::Conversion;
use bijective_risk::valuation::{Pricing, Valuation};
use bijective_venue::profile::Instrument;

use crate::venues::OpenVenues;
use crate::{Fatal, PROGRAM};

const DAY_NS: i128 = 86_400_000_000_000;

/// What the loss is measured against: the declared limits, the venues' instruments, and the
/// pricing the book was valued with.
pub(crate) struct LossInputs<'a> {
    pub(crate) limits: &'a [(Scope, LimitKind, Threshold)],
    pub(crate) adapters: &'a OpenVenues,
    pub(crate) pricing: Pricing<'a>,
}

/// Without a figure the declared limit is unjudged and the node halts, which is the intended
/// failure: a loss that cannot be measured is not a loss within bounds. Nothing is attached when
/// the book could not be valued, because the unrealised part is then unknown. Returns how many of
/// today's fees the figure leaves out. Zero where no figure was measured.
pub(crate) fn attach_daily_loss(
    path: &mut OrderPath,
    inputs: &LossInputs<'_>,
    valued: &mut Valuation,
    now_ns: i128,
) -> Result<u64, Fatal> {
    let declared = |wanted: LimitKind| inputs.limits.iter().any(|(_, kind, _)| *kind == wanted);
    let (loss_declared, drawdown_declared) =
        (declared(LimitKind::DailyLoss), declared(LimitKind::Drawdown));
    if !loss_declared && !drawdown_declared {
        return Ok(0);
    }
    let Some(unrealized) = valued.unrealized else { return Ok(0) };

    let day = u64::try_from(now_ns / DAY_NS).unwrap_or(0);
    if path.day_baseline(day).is_none() {
        path.open_day(day, unrealized.scaled(), now_ns)
            .map_err(|e| Fatal::failure(format!("the day's baseline was not recorded: {e}")))?;
    }
    let baseline = path.day_baseline(day).map(Amount::from_scaled);
    let day_start = i128::from(day) * DAY_NS;
    let (realized, charged, account): (Vec<_>, Vec<_>, Vec<_>) = path
        .recorded_orders()
        .map(|orders| {
            (
                orders.realized_since(day_start).cloned().collect(),
                orders.charged_since(day_start).cloned().collect(),
                orders.account_charged_since(day_start).cloned().collect(),
            )
        })
        .unwrap_or_default();

    let instruments = by_venue_symbol(inputs.adapters);
    let mut book = Conversion::new(inputs.pricing, valued.currency.as_deref());
    let today = Today { realized: &realized, charged: &charged, account: &account };
    let measured = daily_loss_figure(baseline, unrealized, today, &instruments, &mut book);
    match measured {
        Ok(loss) => {
            if drawdown_declared {
                attach_drawdown(path, day, day_result(&loss), valued, now_ns)?;
            }
            if loss_declared {
                valued.snapshot.figures.push(loss.figure);
            }
            Ok(loss.fees_not_valued)
        }
        Err(why) => {
            eprintln!("{PROGRAM}: the daily loss is unmeasured: {why}");
            Ok(0)
        }
    }
}

/// A new high is recorded before the figure is judged, so a restart measures from it. Without a
/// result nothing is attached, and the declared limit is unjudged.
fn attach_drawdown(
    path: &mut OrderPath,
    day: u64,
    result: Option<Amount>,
    valued: &mut Valuation,
    now_ns: i128,
) -> Result<(), Fatal> {
    let Some(result) = result else { return Ok(()) };
    let peak = path.day_peak(day).map(Amount::from_scaled);
    if result.scaled() > peak.map_or(0, Amount::scaled) {
        path.raise_peak(day, result.scaled(), now_ns)
            .map_err(|e| Fatal::failure(format!("the day's high was not recorded: {e}")))?;
    }
    let peak = path.day_peak(day).map(Amount::from_scaled);
    match drawdown_figure(result, peak) {
        Ok(figure) => valued.snapshot.figures.push(figure),
        Err(why) => eprintln!("{PROGRAM}: the drawdown is unmeasured: {why}"),
    }
    Ok(())
}

fn by_venue_symbol(adapters: &OpenVenues) -> BTreeMap<(String, String), Instrument> {
    let mut priced = BTreeMap::new();
    for (venue, adapter) in adapters {
        for instrument in adapter.declaration().instruments().iter() {
            priced.insert(
                (venue.clone(), instrument.symbol.as_str().to_string()),
                instrument.clone(),
            );
        }
    }
    priced
}
