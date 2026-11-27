// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Exposure at every level below the portfolio, as the control engine last recorded it, against
//! the limits declared at each scope.

use std::collections::BTreeMap;

use bijective_control_engine::wire::{ControlEvent, ScopeFigure};
use bijective_limits::{LimitKind, Scope, Threshold};
use bijective_risk::limit_state::wire::LimitEvent;

use crate::markup::{money, nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

const SCOPE_COLUMNS: &[(&str, Unit)] = &[
    ("scope", Unit::Text),
    ("gross", Unit::Money),
    ("gross limit", Unit::Money),
    ("net", Unit::Money),
    ("net limit", Unit::Money),
    ("recorded at", Unit::Nanoseconds),
];

/// One scope's figures and limits. Absent is unmeasured or undeclared, never zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeRow {
    pub gross: Option<i128>,
    pub net: Option<i128>,
    pub gross_limit: Option<i128>,
    pub net_limit: Option<i128>,
}

#[derive(Default)]
pub struct ScopesPanel {
    pub rows: BTreeMap<Scope, ScopeRow>,
    pub recorded_at_ns: Option<i128>,
}

impl ScopesPanel {
    fn declared(&mut self, scope: &Scope, kind: LimitKind, maximum: &Threshold) {
        let (false, Threshold::Money(amount)) = (*scope == Scope::Portfolio, maximum) else {
            return;
        };
        let row = self.rows.entry(scope.clone()).or_default();
        match kind {
            LimitKind::GrossExposure => row.gross_limit = Some(amount.scaled()),
            LimitKind::NetExposure => row.net_limit = Some(amount.scaled()),
            _ => {}
        }
    }

    /// A record replaces every figure: a scope it does not hold is no longer measured.
    fn valued(&mut self, figures: &[ScopeFigure], at_ns: i128) {
        for row in self.rows.values_mut() {
            row.gross = None;
            row.net = None;
        }
        for figure in figures {
            let row = self.rows.entry(figure.scope.clone()).or_default();
            match figure.kind {
                LimitKind::GrossExposure => row.gross = Some(figure.value_scaled),
                LimitKind::NetExposure => row.net = Some(figure.value_scaled),
                _ => {}
            }
        }
        self.recorded_at_ns = Some(at_ns);
    }

    /// The worst standing of any declared limit: unmeasured or at its maximum is an alarm, past
    /// four fifths asks for attention. Net is judged by its size, as the limit is.
    fn worst(&self) -> Option<(Level, String)> {
        let mut worst: Option<(Level, String)> = None;
        for (scope, row) in &self.rows {
            for (name, figure, limit) in
                [("gross", row.gross, row.gross_limit), ("net", row.net, row.net_limit)]
            {
                let Some(limit) = limit else { continue };
                let (level, value) = match figure {
                    None => (Level::Alarm, "unmeasured".to_string()),
                    Some(f) if f.abs() >= limit => (Level::Alarm, money(f)),
                    Some(f) if f.abs() * 5 > limit * 4 => (Level::Attention, money(f)),
                    Some(_) => continue,
                };
                if worst.as_ref().is_none_or(|(w, _)| rank(level) > rank(*w)) {
                    worst = Some((
                        level,
                        format!("{} {name} {value} of {}", spelt(scope), money(limit)),
                    ));
                }
            }
        }
        worst
    }
}

fn rank(level: Level) -> u8 {
    match level {
        Level::Normal => 0,
        Level::Attention => 1,
        Level::Alarm => 2,
    }
}

/// The scope as an operator declares it in configuration.
pub(crate) fn spelt(scope: &Scope) -> String {
    match scope {
        Scope::Portfolio => "portfolio".into(),
        Scope::Desk(n) => format!("desk:{n}"),
        Scope::Venue(n) => format!("venue:{n}"),
        Scope::Strategy(n) => format!("strategy:{n}"),
        Scope::Underlying(n) => format!("underlying:{n}"),
        Scope::Instrument(n) => format!("instrument:{n}"),
    }
}

impl Panel for ScopesPanel {
    fn title(&self) -> &'static str {
        "Exposure by scope"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        match entry {
            Entry::Limit(LimitEvent::Declared { scope, kind, maximum, .. }) => {
                self.declared(scope, *kind, maximum);
            }
            Entry::Control(ControlEvent::ScopesValued { figures, at_ns }) => {
                self.valued(figures, *at_ns);
            }
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        if self.rows.is_empty() {
            return nothing(out, "No figure below the portfolio has been recorded yet.");
        }
        let shown = |f: Option<i128>, declared: bool| match (f, declared) {
            (Some(f), _) => money(f),
            (None, true) => "<strong>unmeasured</strong>".into(),
            (None, false) => String::new(),
        };
        let limit = |l: Option<i128>| l.map_or_else(String::new, money);
        table_head(out, &["scope", "gross", "gross limit", "net", "net limit"]);
        for (scope, row) in &self.rows {
            let declared = row.gross_limit.is_some() || row.net_limit.is_some();
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{}</tr>",
                text(&spelt(scope)),
                shown(row.gross, declared),
                limit(row.gross_limit),
                shown(row.net, declared),
                limit(row.net_limit),
            ));
        }
        out.push_str("</table>");
        let at = self.recorded_at_ns.map_or_else(|| "never".to_string(), |t| t.to_string());
        out.push_str(&format!(
            "<p class=\"none\">In the book's settlement currency. Recorded at {at}. Desk and \
             strategy figures are bounds from the platform's fills and working orders, with \
             anything the record does not explain counted against each.</p>"
        ));
    }

    fn signals(&self) -> Vec<Signal> {
        self.worst()
            .map(|(level, value)| Signal { label: "scope limits", value, level })
            .into_iter()
            .collect()
    }

    /// An absent figure is unmeasured where its scope declares a limit, and not tracked where it
    /// declares none. The limit columns tell which.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(SCOPE_COLUMNS);
        for (scope, row) in &self.rows {
            table.rows.push(vec![
                Cell::text(spelt(scope)),
                Cell::integer(row.gross),
                Cell::integer(row.gross_limit),
                Cell::integer(row.net),
                Cell::integer(row.net_limit),
                Cell::integer(self.recorded_at_ns),
            ]);
        }
        Some(table)
    }
}
