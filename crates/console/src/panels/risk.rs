// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Limits in force, the safe states driven, and whether each was honoured.

use std::collections::BTreeMap;

use bijective_control_engine::wire::{ControlEvent, PostconditionOutcome};
use bijective_limits::{LimitKind, SafeState, Scope, Threshold};
use bijective_risk::limit_state::wire::{LimitEvent, ReleaseReason};

use super::keep;
use super::scopes::spelt;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, MONEY_SCALE, RATIO_SCALE, Table, Unit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitRow {
    pub scope: Scope,
    pub kind: LimitKind,
    pub maximum: Threshold,
    pub consumed: usize,
    pub released: usize,
    pub last_release: Option<ReleaseReason>,
}

#[derive(Default)]
pub struct LimitsPanel {
    pub limits: BTreeMap<(String, LimitKind), LimitRow>,
}

impl Panel for LimitsPanel {
    fn title(&self) -> &'static str {
        "Limits"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Limit(event) = entry else { return };
        match event {
            LimitEvent::Declared { scope, kind, maximum, .. } => {
                let row = LimitRow {
                    scope: scope.clone(),
                    kind: *kind,
                    maximum: *maximum,
                    consumed: 0,
                    released: 0,
                    last_release: None,
                };
                self.limits.insert((format!("{scope:?}"), *kind), row);
            }
            LimitEvent::Consumed { scope, kind, .. } => {
                if let Some(row) = self.limits.get_mut(&(format!("{scope:?}"), *kind)) {
                    row.consumed += 1;
                }
            }
            LimitEvent::Released { scope, kind, reason, .. } => {
                if let Some(row) = self.limits.get_mut(&(format!("{scope:?}"), *kind)) {
                    row.released += 1;
                    row.last_release = Some(*reason);
                }
            }
        }
    }

    fn render(&self, out: &mut String) {
        if self.limits.is_empty() {
            return nothing(out, "No limit has entered force on this stream.");
        }
        table_head(out, &["scope", "kind", "maximum", "consumed", "released", "last release"]);
        for l in self.limits.values() {
            let last = l.last_release.map_or_else(String::new, |r| text(&format!("{r:?}")));
            out.push_str(&format!(
                "<tr><td>{}<td>{:?}<td>{}<td>{}<td>{}<td>{last}</tr>",
                text(&format!("{:?}", l.scope)),
                l.kind,
                text(&format!("{:?}", l.maximum)),
                l.consumed,
                l.released
            ));
        }
        out.push_str("</table>");
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("scope", Unit::Text),
            ("limit", Unit::Text),
            ("maximum", Unit::Decimal),
            ("unit", Unit::Text),
            ("consumed", Unit::Count),
            ("released", Unit::Count),
            ("last release", Unit::Text),
        ]);
        for l in self.limits.values() {
            let last = l.last_release.map(|r| format!("{r:?}"));
            table.rows.push(vec![
                Cell::text(spelt(&l.scope)),
                Cell::text(format!("{:?}", l.kind)),
                threshold(l.maximum),
                Cell::text(threshold_unit(l.maximum)),
                Cell::count(l.consumed),
                Cell::count(l.released),
                Cell::optional_text(last.as_deref()),
            ]);
        }
        Some(table)
    }
}

/// A threshold as the exact decimal it stands for, money at `MONEY_SCALE` and a ratio at
/// `RATIO_SCALE`. `threshold_unit` names which.
fn threshold(value: Threshold) -> Cell {
    match value {
        Threshold::Money(amount) => Cell::decimal(Some(amount.scaled()), Some(MONEY_SCALE)),
        Threshold::Ratio(ratio) => Cell::decimal(Some(ratio.scaled()), Some(RATIO_SCALE)),
    }
}

fn threshold_unit(value: Threshold) -> &'static str {
    match value {
        Threshold::Money(_) => "money",
        Threshold::Ratio(_) => "ratio",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeStateRow {
    pub target: SafeState,
    pub arrived: bool,
    pub at_ns: i128,
}

/// The safe states driven, and the trading state last recorded, whichever recorded it: a safe
/// state the node drove or a command an operator carried out.
#[derive(Default)]
pub struct SafeStatePanel {
    pub driven: Vec<SafeStateRow>,
    pub trading: Option<SafeState>,
}

impl Panel for SafeStatePanel {
    fn title(&self) -> &'static str {
        "Safe state"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(event) = entry else { return };
        match event {
            ControlEvent::SafeStateDriven { target, arrived, at_ns, .. } => {
                if *arrived {
                    self.trading = Some(*target);
                }
                let row = SafeStateRow { target: *target, arrived: *arrived, at_ns: *at_ns };
                keep(&mut self.driven, row);
            }
            ControlEvent::CommandApplied { state, .. } => self.trading = Some(*state),
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        if self.driven.is_empty() {
            return nothing(out, "No safe state has been driven.");
        }
        table_head(out, &["target", "arrived", "at"]);
        for s in &self.driven {
            let arrived = if s.arrived { "yes" } else { "<strong>no</strong>" };
            out.push_str(&format!("<tr><td>{:?}<td>{arrived}<td>{}</tr>", s.target, s.at_ns));
        }
        out.push_str("</table>");
    }

    /// A node always starts halted, so a halt is shown as needing attention, not as an alarm.
    fn signals(&self) -> Vec<Signal> {
        let (value, level) = match self.trading {
            None => ("halted (nothing recorded yet)".to_string(), Level::Attention),
            Some(SafeState::Active) => ("active".to_string(), Level::Normal),
            Some(state) => (format!("{state:?}").to_lowercase(), Level::Attention),
        };
        vec![Signal { label: "trading", value, level }]
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("target", Unit::Text),
            ("arrived", Unit::Flag),
            ("at", Unit::Nanoseconds),
        ]);
        for s in &self.driven {
            let target = format!("{:?}", s.target).to_lowercase();
            table.rows.push(vec![
                Cell::text(target),
                Cell::Flag(s.arrived),
                Cell::Integer(s.at_ns),
            ]);
        }
        Some(table)
    }
}

/// Whether the figure a safe state was driven on did what the instruction was for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostconditionRow {
    pub scope: Scope,
    pub kind: LimitKind,
    pub before: Threshold,
    pub outcome: PostconditionOutcome,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct PostconditionsPanel {
    pub answered: Vec<PostconditionRow>,
}

impl Panel for PostconditionsPanel {
    fn title(&self) -> &'static str {
        "Safe state honoured"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::SafeStatePostcondition {
            scope,
            kind,
            before,
            outcome,
            at_ns,
        }) = entry
        else {
            return;
        };
        let row = PostconditionRow {
            scope: scope.clone(),
            kind: *kind,
            before: *before,
            outcome: *outcome,
            at_ns: *at_ns,
        };
        keep(&mut self.answered, row);
    }

    fn render(&self, out: &mut String) {
        if self.answered.is_empty() {
            return nothing(out, "No safe state has been answered.");
        }
        table_head(out, &["scope", "limit", "before", "after", "at"]);
        for p in &self.answered {
            // A halt the venue took and then grew through must not look like every other row.
            let after = match &p.outcome {
                PostconditionOutcome::Held { after } => text(&format!("{after:?}")),
                PostconditionOutcome::Increased { after } => {
                    format!("<strong>grew to {}</strong>", text(&format!("{after:?}")))
                }
                PostconditionOutcome::Unverifiable => {
                    "<strong>no figure arrived</strong>".to_string()
                }
            };
            out.push_str(&format!(
                "<tr><td>{}<td>{:?}<td>{}<td>{after}<td>{}</tr>",
                text(&format!("{:?}", p.scope)),
                p.kind,
                text(&format!("{:?}", p.before)),
                p.at_ns
            ));
        }
        out.push_str("</table>");
    }

    fn signals(&self) -> Vec<Signal> {
        let grew = self
            .answered
            .iter()
            .filter(|p| matches!(p.outcome, PostconditionOutcome::Increased { .. }))
            .count();
        if grew == 0 {
            return Vec::new();
        }
        let value = format!("{grew} safe state(s) where exposure grew");
        vec![Signal { label: "safe state", value, level: Level::Alarm }]
    }

    /// `after` is absent where no figure arrived to judge the safe state by.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("scope", Unit::Text),
            ("limit", Unit::Text),
            ("before", Unit::Decimal),
            ("outcome", Unit::Text),
            ("after", Unit::Decimal),
            ("unit", Unit::Text),
            ("at", Unit::Nanoseconds),
        ]);
        for p in &self.answered {
            let (outcome, after) = match p.outcome {
                PostconditionOutcome::Held { after } => ("held", threshold(after)),
                PostconditionOutcome::Increased { after } => ("increased", threshold(after)),
                PostconditionOutcome::Unverifiable => ("unverifiable", Cell::Absent),
            };
            table.rows.push(vec![
                Cell::text(spelt(&p.scope)),
                Cell::text(format!("{:?}", p.kind)),
                threshold(p.before),
                Cell::text(outcome),
                after,
                Cell::text(threshold_unit(p.before)),
                Cell::Integer(p.at_ns),
            ]);
        }
        Some(table)
    }
}
