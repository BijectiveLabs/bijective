// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Which venues the node routes to, and what each reconciliation pass concluded.

use std::collections::BTreeMap;

use bijective_control_engine::wire::{ControlEvent, EvidenceRead, Participation};
use bijective_lifecycle::Verdict;

use super::keep;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueRow {
    pub participation: Participation,
    /// When it last changed, so an operator asking *since when* has an answer.
    pub since_ns: i128,
    pub changes: usize,
}

#[derive(Default)]
pub struct VenuesPanel {
    pub venues: BTreeMap<String, VenueRow>,
}

impl Panel for VenuesPanel {
    fn title(&self) -> &'static str {
        "Venues"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::VenueParticipation { venue, participation, at_ns }) =
            entry
        else {
            return;
        };
        let row = self.venues.entry(venue.clone()).or_insert(VenueRow {
            participation: participation.clone(),
            since_ns: *at_ns,
            changes: 0,
        });
        row.participation = participation.clone();
        row.since_ns = *at_ns;
        row.changes += 1;
    }

    fn render(&self, out: &mut String) {
        if self.venues.is_empty() {
            return nothing(out, "No venue has joined or left since this stream began.");
        }
        table_head(out, &["venue", "participation", "since", "changes"]);
        for (venue, v) in &self.venues {
            let state = match &v.participation {
                Participation::Participating => "participating".to_string(),
                Participation::Withdrawn { reason } => {
                    format!("<strong>withdrawn</strong> - {}", text(reason))
                }
            };
            out.push_str(&format!(
                "<tr><td>{}<td>{state}<td>{}<td>{}</tr>",
                text(venue),
                v.since_ns,
                v.changes
            ));
        }
        out.push_str("</table>");
    }

    fn signals(&self) -> Vec<Signal> {
        let withdrawn = self
            .venues
            .values()
            .filter(|v| matches!(v.participation, Participation::Withdrawn { .. }))
            .count();
        if withdrawn == 0 {
            return Vec::new();
        }
        let value = withdrawn.to_string();
        vec![Signal { label: "venues withdrawn", value, level: Level::Attention }]
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(VENUE_COLUMNS);
        for (venue, v) in &self.venues {
            let (state, reason) = match &v.participation {
                Participation::Participating => ("participating", None),
                Participation::Withdrawn { reason } => ("withdrawn", Some(reason.as_str())),
            };
            table.rows.push(vec![
                Cell::text(venue.clone()),
                Cell::text(state),
                Cell::optional_text(reason),
                Cell::Integer(v.since_ns),
                Cell::count(v.changes),
            ]);
        }
        Some(table)
    }
}

const VENUE_COLUMNS: &[(&str, Unit)] = &[
    ("venue", Unit::Text),
    ("participation", Unit::Text),
    ("reason", Unit::Text),
    ("since", Unit::Nanoseconds),
    ("changes", Unit::Count),
];

/// A pass whose evidence could not be read is `unread`, whatever verdict it carries.
const RECONCILIATION_COLUMNS: &[(&str, Unit)] = &[
    ("venue", Unit::Text),
    ("verdict", Unit::Text),
    ("findings", Unit::Count),
    ("detail", Unit::Text),
    ("at", Unit::Nanoseconds),
];

/// One reconciliation pass against one venue, with the findings an operator resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationRow {
    pub venue: String,
    pub verdict: Verdict,
    /// False when the venue's evidence could not be read: nothing about it was checked.
    pub read: bool,
    pub findings: Vec<String>,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct ReconciliationPanel {
    /// The latest passes, newest last.
    pub passes: Vec<ReconciliationRow>,
    /// The latest pass per venue, for the strip.
    latest: BTreeMap<String, (Verdict, bool)>,
}

impl Panel for ReconciliationPanel {
    fn title(&self) -> &'static str {
        "Reconciliation"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::ReconciliationCompleted {
            venue,
            verdict,
            findings,
            evidence,
            at_ns,
        }) = entry
        else {
            return;
        };
        let read = *evidence != EvidenceRead::Failed;
        self.latest.insert(venue.clone(), (*verdict, read));
        let row = ReconciliationRow {
            venue: venue.clone(),
            verdict: *verdict,
            read,
            findings: findings.clone(),
            at_ns: *at_ns,
        };
        keep(&mut self.passes, row);
    }

    fn render(&self, out: &mut String) {
        if self.passes.is_empty() {
            return nothing(out, "No reconciliation pass is recorded.");
        }
        table_head(out, &["venue", "verdict", "findings", "at"]);
        for r in &self.passes {
            let verdict = match r.verdict {
                _ if !r.read => "<strong>unread</strong>".to_string(),
                Verdict::Aligned => "aligned".to_string(),
                Verdict::Unverified => "unverified".to_string(),
                Verdict::Diverged => "<strong>diverged</strong>".to_string(),
            };
            let findings: Vec<String> = r.findings.iter().map(|f| text(f)).collect();
            out.push_str(&format!(
                "<tr><td>{}<td>{verdict}<td>{}<td>{}</tr>",
                text(&r.venue),
                findings.join("<br>"),
                r.at_ns
            ));
        }
        out.push_str("</table>");
    }

    fn signals(&self) -> Vec<Signal> {
        let unsettled: Vec<&str> = self
            .latest
            .iter()
            .filter(|(_, (verdict, read))| !read || *verdict == Verdict::Diverged)
            .map(|(venue, _)| venue.as_str())
            .collect();
        if unsettled.is_empty() {
            return Vec::new();
        }
        let value = format!("diverged or unread on {}", unsettled.join(", "));
        vec![Signal { label: "reconciliation", value, level: Level::Alarm }]
    }

    /// One row per pass, oldest first. The findings' sentences joined by a line break.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(RECONCILIATION_COLUMNS);
        for r in &self.passes {
            let verdict = match r.verdict {
                _ if !r.read => "unread",
                Verdict::Aligned => "aligned",
                Verdict::Unverified => "unverified",
                Verdict::Diverged => "diverged",
            };
            table.rows.push(vec![
                Cell::text(r.venue.clone()),
                Cell::text(verdict),
                Cell::count(r.findings.len()),
                Cell::text(r.findings.join("\n")),
                Cell::Integer(r.at_ns),
            ]);
        }
        Some(table)
    }
}
