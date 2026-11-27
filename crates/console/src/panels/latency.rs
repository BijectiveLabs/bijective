// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! How long each venue takes to answer an order, as this node measured it.
//!
//! The span is from the intent, written just before the venue call, to the settlement, written
//! once the venue answered: signing, the network both ways and the venue's own processing. It is
//! the latency a strategy's order meets, not the venue's matching time alone.

use std::collections::BTreeMap;

use bijective_control_engine::wire::ControlEvent;

use super::TAIL;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Panel, Signal};
use crate::table::{Cell, Table, Unit};

/// One venue's recent answers, newest last, in nanoseconds.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VenueLatency {
    pub spans_ns: Vec<i128>,
    /// Every answer since the stream began. The spans are only the newest `TAIL`.
    pub answered: usize,
}

impl VenueLatency {
    /// The nearest-rank percentile of the kept spans, `None` while there are none.
    #[must_use]
    pub fn percentile(&self, percent: usize) -> Option<i128> {
        let mut sorted = self.spans_ns.clone();
        sorted.sort_unstable();
        let rank = (percent * sorted.len()).div_ceil(100).max(1);
        sorted.get(rank - 1).copied()
    }
}

#[derive(Default)]
pub struct LatencyPanel {
    pub venues: BTreeMap<String, VenueLatency>,
    /// Intents not yet answered: the venue each was sent to and when.
    pending: BTreeMap<String, (String, i128)>,
    /// Answers to intents recorded before intents named their venue. Counted, never guessed.
    pub unattributed: usize,
}

impl LatencyPanel {
    /// Intents still waiting for an answer.
    #[must_use]
    pub fn unanswered(&self) -> usize {
        self.pending.len()
    }
}

impl Panel for LatencyPanel {
    fn title(&self) -> &'static str {
        "Venue latency"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(event) = entry else { return };
        match event {
            ControlEvent::Intent { venue: Some(venue), request, at_ns, .. } => {
                self.pending.insert(request.client_id.clone(), (venue.clone(), *at_ns));
            }
            ControlEvent::Intent { venue: None, .. } => self.unattributed += 1,
            // Settled by reconciliation, not answered: no span was measured, and it waits no more.
            ControlEvent::Reconciled { client_id, .. } => {
                self.pending.remove(client_id);
            }
            ControlEvent::Settled { client_id, at_ns, .. } => {
                let Some((venue, sent_ns)) = self.pending.remove(client_id) else { return };
                let row = self.venues.entry(venue).or_default();
                row.answered += 1;
                row.spans_ns.push(at_ns - sent_ns);
                if row.spans_ns.len() > TAIL {
                    row.spans_ns.remove(0);
                }
            }
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        if self.venues.is_empty() {
            return nothing(out, "No venue has answered an order since this stream began.");
        }
        table_head(out, &["venue", "answers", "p50 ms", "p95 ms", "max ms"]);
        for (venue, row) in &self.venues {
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{}</tr>",
                text(venue),
                row.answered,
                millis(row.percentile(50)),
                millis(row.percentile(95)),
                millis(row.percentile(100)),
            ));
        }
        out.push_str("</table>");
        out.push_str(&format!(
            "<p class=\"none\">Over the newest {TAIL} answers per venue; {} awaiting an answer, {} \
             from intents that did not name their venue.</p>",
            self.unanswered(),
            self.unattributed
        ));
    }

    fn signals(&self) -> Vec<Signal> {
        // No latency bound is declared anywhere, so the panel informs and never alarms.
        Vec::new()
    }

    /// Per venue over the kept spans, as the page shows them. The awaiting and unattributed counts
    /// are the page's footnote and not a venue's.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("venue", Unit::Text),
            ("answers", Unit::Count),
            ("p50", Unit::Span),
            ("p95", Unit::Span),
            ("max", Unit::Span),
        ]);
        for (venue, row) in &self.venues {
            table.rows.push(vec![
                Cell::text(venue.clone()),
                Cell::count(row.answered),
                Cell::integer(row.percentile(50)),
                Cell::integer(row.percentile(95)),
                Cell::integer(row.percentile(100)),
            ]);
        }
        Some(table)
    }
}

/// Milliseconds with one decimal, from nanoseconds. A dash where nothing was measured.
fn millis(span_ns: Option<i128>) -> String {
    match span_ns {
        Some(ns) => format!("{}.{}", ns / 1_000_000, (ns % 1_000_000).abs() / 100_000),
        None => "-".to_string(),
    }
}
