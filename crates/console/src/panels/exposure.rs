// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The portfolio's figures as the control engine last recorded them, against their limits.

use bijective_control_engine::wire::{ControlEvent, Valued};
use bijective_limits::{LimitKind, Ratio, Scope, Threshold};
use bijective_risk::limit_state::wire::LimitEvent;

use crate::markup::{money, nothing, sparkline, table_head};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

const PORTFOLIO_COLUMNS: &[(&str, Unit)] = &[
    ("recorded at", Unit::Nanoseconds),
    ("observed at", Unit::Nanoseconds),
    ("gross exposure", Unit::Money),
    ("gross exposure limit", Unit::Money),
    ("net exposure", Unit::Money),
    ("daily loss", Unit::Money),
    ("daily loss limit", Unit::Money),
    ("leverage", Unit::Ratio),
    ("leverage limit", Unit::Ratio),
    ("margin use", Unit::Ratio),
    ("margin use limit", Unit::Ratio),
    ("equity", Unit::Money),
];

/// Points kept for the gross exposure line.
const HISTORY: usize = 240;

#[derive(Default)]
pub struct ExposurePanel {
    pub latest: Option<(Valued, i128)>,
    /// The newest `HISTORY` records, oldest first, each with the limits in force when it was
    /// recorded, so a past figure is judged against the limit of its own time.
    pub history: Vec<Recorded>,
    pub gross_limit: Option<i128>,
    pub daily_loss_limit: Option<i128>,
    /// At `Ratio::ONE`, as declared.
    pub leverage_limit: Option<i128>,
    pub margin_use_limit: Option<i128>,
}

impl ExposurePanel {
    fn declared(&mut self, scope: &Scope, kind: LimitKind, maximum: &Threshold) {
        if *scope != Scope::Portfolio {
            return;
        }
        match (kind, maximum) {
            (LimitKind::GrossExposure, Threshold::Money(a)) => self.gross_limit = Some(a.scaled()),
            (LimitKind::DailyLoss, Threshold::Money(a)) => self.daily_loss_limit = Some(a.scaled()),
            (LimitKind::Leverage, Threshold::Ratio(r)) => self.leverage_limit = Some(r.scaled()),
            (LimitKind::MarginUse, Threshold::Ratio(r)) => self.margin_use_limit = Some(r.scaled()),
            _ => {}
        }
    }
}

/// A ratio at `Ratio::ONE` as a multiple with two decimals: 25000 is `2.50x`.
fn times(scaled: i128) -> String {
    let hundredths = scaled * 100 / Ratio::ONE;
    let sign = if hundredths < 0 { "-" } else { "" };
    let magnitude = hundredths.unsigned_abs();
    format!("{sign}{}.{:02}x", magnitude / 100, magnitude % 100)
}

/// A ratio at `Ratio::ONE` as a percentage with two decimals: 3500 is `35.00%`.
fn percent(scaled: i128) -> String {
    let sign = if scaled < 0 { "-" } else { "" };
    let magnitude = scaled.unsigned_abs();
    format!("{sign}{}.{:02}%", magnitude / 100, magnitude % 100)
}

/// A figure against its limit: at or past the limit is an alarm, past four fifths of it asks for
/// attention, and a limit whose figure is missing is an alarm, because the limit cannot be judged.
fn against(
    label: &'static str,
    figure: Option<i128>,
    limit: Option<i128>,
    shown: fn(i128) -> String,
) -> Option<Signal> {
    let signal = |value: String, level| Some(Signal { label, value, level });
    match (figure, limit) {
        (None, None) => None,
        (None, Some(_)) => signal("unmeasured".into(), Level::Alarm),
        (Some(f), None) => signal(shown(f), Level::Normal),
        (Some(f), Some(l)) => {
            let level = if f >= l {
                Level::Alarm
            } else if f * 5 > l * 4 {
                Level::Attention
            } else {
                Level::Normal
            };
            signal(format!("{} of {}", shown(f), shown(l)), level)
        }
    }
}

fn row(
    out: &mut String,
    name: &str,
    figure: Option<i128>,
    limit: Option<i128>,
    shown: fn(i128) -> String,
) {
    let value = figure.map_or_else(|| "<strong>unmeasured</strong>".into(), shown);
    let limit = limit.map_or_else(|| "<span class=\"none\">none declared</span>".into(), shown);
    out.push_str(&format!("<tr><td>{name}<td>{value}<td>{limit}</tr>"));
}

impl Panel for ExposurePanel {
    fn title(&self) -> &'static str {
        "Portfolio"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        match entry {
            Entry::Limit(LimitEvent::Declared { scope, kind, maximum, .. }) => {
                self.declared(scope, *kind, maximum);
            }
            Entry::Control(ControlEvent::PortfolioValued { figures, at_ns }) => {
                self.latest = Some((*figures, *at_ns));
                let limits = [
                    self.gross_limit,
                    self.daily_loss_limit,
                    self.leverage_limit,
                    self.margin_use_limit,
                ];
                self.history.push(Recorded { at_ns: *at_ns, figures: *figures, limits });
                if self.history.len() > HISTORY {
                    self.history.remove(0);
                }
            }
            _ => {}
        }
    }

    /// The ratio rows show only where a figure or a limit exists: nothing declared them otherwise.
    fn render(&self, out: &mut String) {
        let Some((figures, at_ns)) = &self.latest else {
            return nothing(out, "No portfolio figure has been recorded yet.");
        };
        table_head(out, &["figure", "value", "limit"]);
        row(out, "gross exposure", figures.gross_scaled, self.gross_limit, money);
        out.push_str(&format!(
            "<tr><td>net exposure<td>{}<td></tr>",
            figures.net_scaled.map_or_else(|| "<strong>unmeasured</strong>".into(), money)
        ));
        row(out, "daily loss", figures.daily_loss_scaled, self.daily_loss_limit, money);
        if figures.leverage_scaled.is_some() || self.leverage_limit.is_some() {
            row(out, "leverage", figures.leverage_scaled, self.leverage_limit, times);
        }
        if figures.margin_use_scaled.is_some() || self.margin_use_limit.is_some() {
            row(out, "margin use", figures.margin_use_scaled, self.margin_use_limit, percent);
        }
        if let Some(equity) = figures.equity_scaled {
            out.push_str(&format!("<tr><td>equity<td>{}<td></tr>", money(equity)));
        }
        out.push_str("</table>");
        out.push_str(&format!(
            "<p class=\"none\">In the book's settlement currency. Recorded at {at_ns}, from inputs \
             observed at {}. A summary is recorded when a figure moves by a hundredth or at most \
             once a minute.</p>",
            figures.observed_at_ns
        ));
        let gross: Vec<i128> = self.history.iter().filter_map(|r| r.figures.gross_scaled).collect();
        out.push_str(&sparkline(&gross));
    }

    fn signals(&self) -> Vec<Signal> {
        let figures = self.latest.map(|(f, _)| f);
        let loss = figures.and_then(|f| f.daily_loss_scaled);
        let gross = figures.and_then(|f| f.gross_scaled);
        let leverage = figures.and_then(|f| f.leverage_scaled);
        let margin_use = figures.and_then(|f| f.margin_use_scaled);
        [
            against("daily loss", loss, self.daily_loss_limit, money),
            against("gross exposure", gross, self.gross_limit, money),
            against("leverage", leverage, self.leverage_limit, times),
            against("margin use", margin_use, self.margin_use_limit, percent),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// One row per kept record, oldest first. Each carries the limits in force when it was
    /// recorded, except the newest, which carries those in force now, as the page and the strip
    /// judge it. An absent figure is unmeasured and an absent limit undeclared.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(PORTFOLIO_COLUMNS);
        let now =
            [self.gross_limit, self.daily_loss_limit, self.leverage_limit, self.margin_use_limit];
        let newest = self.history.len().saturating_sub(1);
        for (i, r) in self.history.iter().enumerate() {
            let [gross, loss, leverage, margin] = if i == newest { now } else { r.limits };
            let f = &r.figures;
            table.rows.push(vec![
                Cell::Integer(r.at_ns),
                Cell::Integer(f.observed_at_ns),
                Cell::integer(f.gross_scaled),
                Cell::integer(gross),
                Cell::integer(f.net_scaled),
                Cell::integer(f.daily_loss_scaled),
                Cell::integer(loss),
                Cell::integer(f.leverage_scaled),
                Cell::integer(leverage),
                Cell::integer(f.margin_use_scaled),
                Cell::integer(margin),
                Cell::integer(f.equity_scaled),
            ]);
        }
        Some(table)
    }
}

/// One portfolio record as the panel keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    pub at_ns: i128,
    pub figures: Valued,
    /// Gross exposure, daily loss, leverage and margin use limits, in that order.
    pub limits: [Option<i128>; 4],
}
