// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What detectors concluded, from the `detections` stream: each subject's standing conclusion,
//! and the conclusions as they were written.
//!
//! The console runs no detector. It shows what any program wrote in the open format
//! (`crate::detection`), so an operator's own detector is shown the same way as a vendor's.

use std::collections::BTreeMap;

use super::keep;
use crate::detection::{Detection, Raised};
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

const DETECTION_COLUMNS: &[(&str, Unit)] = &[
    ("at", Unit::Nanoseconds),
    ("detector", Unit::Text),
    ("subject", Unit::Text),
    ("level", Unit::Text),
    ("measured", Unit::Decimal),
    ("threshold", Unit::Decimal),
    ("detail", Unit::Text),
    ("source", Unit::Text),
    ("evidence", Unit::Text),
];

fn cells(d: &Detection) -> Vec<Cell> {
    let evidence: Vec<String> = d.evidence.iter().map(u64::to_string).collect();
    vec![
        Cell::Integer(d.at_ns),
        Cell::text(d.detector.clone()),
        Cell::text(d.subject.clone()),
        Cell::text(d.level.word()),
        Cell::optional_text(d.measured.as_deref()),
        Cell::optional_text(d.threshold.as_deref()),
        Cell::text(d.detail.clone()),
        Cell::text(d.source.clone()),
        Cell::text(evidence.join(" ")),
    ]
}

fn render_rows<'a>(out: &mut String, rows: impl Iterator<Item = &'a Detection>) {
    table_head(out, &["at", "detector", "subject", "level", "measured", "threshold", "detail"]);
    for d in rows {
        let level = match d.level {
            Raised::Alarm => "<strong>alarm</strong>",
            other => other.word(),
        };
        out.push_str(&format!(
            "<tr><td>{}<td>{}<td>{}<td>{level}<td>{}<td>{}<td>{}</tr>",
            d.at_ns,
            text(&d.detector),
            text(&d.subject),
            text(d.measured.as_deref().unwrap_or("-")),
            text(d.threshold.as_deref().unwrap_or("-")),
            text(&d.detail),
        ));
    }
    out.push_str("</table>");
}

/// Each detector's latest conclusion per subject that still holds: a cleared one leaves.
#[derive(Default)]
pub struct StandingDetectionsPanel {
    pub standing: BTreeMap<(String, String), Detection>,
}

impl Panel for StandingDetectionsPanel {
    fn title(&self) -> &'static str {
        "Open detections"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Detection(d) = entry else { return };
        let key = (d.detector.clone(), d.subject.clone());
        if d.level == Raised::Cleared {
            self.standing.remove(&key);
        } else {
            self.standing.insert(key, (*d).clone());
        }
    }

    fn render(&self, out: &mut String) {
        if self.standing.is_empty() {
            return nothing(out, "No detector holds a conclusion open.");
        }
        render_rows(out, self.standing.values());
    }

    fn signals(&self) -> Vec<Signal> {
        let count = |level| self.standing.values().filter(|d| d.level == level).count();
        let (alarms, attention) = (count(Raised::Alarm), count(Raised::Attention));
        let (value, level) = match (alarms, attention) {
            (0, 0) => return Vec::new(),
            (0, n) => (format!("{n} open"), Level::Attention),
            (a, n) => (format!("{a} alarm, {n} attention"), Level::Alarm),
        };
        vec![Signal { label: "detections", value, level }]
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(DETECTION_COLUMNS);
        table.rows.extend(self.standing.values().map(cells));
        Some(table)
    }
}

/// The conclusions as written, the newest `TAIL`, oldest first.
#[derive(Default)]
pub struct DetectionsPanel {
    pub written: Vec<Detection>,
    pub seen: usize,
}

impl Panel for DetectionsPanel {
    fn title(&self) -> &'static str {
        "Detections"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Detection(d) = entry else { return };
        self.seen += 1;
        keep(&mut self.written, (*d).clone());
    }

    fn render(&self, out: &mut String) {
        out.push_str(&format!(
            "<p class=\"none\">{} written, {} shown. No detector runs in the console: these are \
             what the programs writing the detections stream concluded.</p>",
            self.seen,
            self.written.len()
        ));
        if !self.written.is_empty() {
            render_rows(out, self.written.iter().rev());
        }
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(DETECTION_COLUMNS);
        table.rows.extend(self.written.iter().map(cells));
        Some(table)
    }
}
