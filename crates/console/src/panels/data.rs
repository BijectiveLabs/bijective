// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The health of the data: each venue link's history, and entries this build could not read.

use std::collections::BTreeMap;

use bijective_venue::contract::ConnectionState;

use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Level, Panel, Signal, Unreadable};
use crate::table::{Cell, Table, Unit};

/// One venue's link. Reconnects are counted, not inferred: connected now can hide forty reconnects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRow {
    pub state: ConnectionState,
    /// When this platform observed the current state, by its own clock.
    pub since_ns: i128,
    pub transitions: usize,
    pub reconnects: usize,
    pub degraded: usize,
}

#[derive(Default)]
pub struct LinksPanel {
    /// By venue, discovered from stream names, so there is no venue list to get wrong.
    pub links: BTreeMap<String, LinkRow>,
}

impl Panel for LinksPanel {
    fn title(&self) -> &'static str {
        "Links"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Link { venue, transition } = entry else { return };
        let connected = matches!(transition.state, ConnectionState::Connected { .. });
        let degraded = usize::from(matches!(transition.state, ConnectionState::Degraded { .. }));
        let Some(held) = self.links.get_mut(*venue) else {
            let row = LinkRow {
                state: transition.state.clone(),
                since_ns: transition.at_ns,
                transitions: 1,
                reconnects: 0,
                degraded,
            };
            self.links.insert((*venue).to_string(), row);
            return;
        };
        if connected && !matches!(held.state, ConnectionState::Connected { .. }) {
            held.reconnects += 1;
        }
        held.degraded += degraded;
        held.state = transition.state.clone();
        held.since_ns = transition.at_ns;
        held.transitions += 1;
    }

    fn render(&self, out: &mut String) {
        if self.links.is_empty() {
            return nothing(out, "No venue has reported a link state.");
        }
        table_head(out, &["venue", "state", "since", "transitions", "reconnects", "degraded"]);
        for (venue, l) in &self.links {
            let state = match &l.state {
                ConnectionState::Connected { .. } => "connected".to_string(),
                ConnectionState::Connecting => "<em>connecting</em>".to_string(),
                ConnectionState::Disconnected => "<strong>disconnected</strong>".to_string(),
                ConnectionState::Degraded { reason, .. } => {
                    format!("<strong>degraded</strong> - {}", text(&format!("{reason:?}")))
                }
            };
            // Beside the state: a link connected now may have reconnected forty times in an hour.
            let churn = if l.reconnects > 1 {
                format!("<strong>{}</strong>", l.reconnects)
            } else {
                l.reconnects.to_string()
            };
            out.push_str(&format!(
                "<tr><td>{}<td>{state}<td>{}<td>{}<td>{churn}<td>{}</tr>",
                text(venue),
                l.since_ns,
                l.transitions,
                l.degraded
            ));
        }
        out.push_str("</table>");
    }

    fn signals(&self) -> Vec<Signal> {
        let down = self
            .links
            .values()
            .filter(|l| !matches!(l.state, ConnectionState::Connected { .. }))
            .count();
        if self.links.is_empty() {
            return Vec::new();
        }
        let level = if down == 0 { Level::Normal } else { Level::Attention };
        let value = format!("{} of {} connected", self.links.len() - down, self.links.len());
        vec![Signal { label: "links", value, level }]
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(LINK_COLUMNS);
        for (venue, l) in &self.links {
            let (state, reason) = match &l.state {
                ConnectionState::Connected { .. } => ("connected", None),
                ConnectionState::Connecting => ("connecting", None),
                ConnectionState::Disconnected => ("disconnected", None),
                ConnectionState::Degraded { reason, .. } => {
                    ("degraded", Some(format!("{reason:?}")))
                }
            };
            table.rows.push(vec![
                Cell::text(venue.clone()),
                Cell::text(state),
                Cell::optional_text(reason.as_deref()),
                Cell::Integer(l.since_ns),
                Cell::count(l.transitions),
                Cell::count(l.reconnects),
                Cell::count(l.degraded),
            ]);
        }
        Some(table)
    }
}

const LINK_COLUMNS: &[(&str, Unit)] = &[
    ("venue", Unit::Text),
    ("state", Unit::Text),
    ("reason", Unit::Text),
    ("since", Unit::Nanoseconds),
    ("transitions", Unit::Count),
    ("reconnects", Unit::Count),
    ("degraded", Unit::Count),
];

#[derive(Default)]
pub struct UnreadablePanel {
    pub unreadable: Vec<Unreadable>,
}

impl Panel for UnreadablePanel {
    fn title(&self) -> &'static str {
        "Entries this build could not read"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        if let Entry::Unreadable(u) = entry {
            self.unreadable.push((*u).clone());
        }
    }

    fn render(&self, out: &mut String) {
        if self.unreadable.is_empty() {
            return nothing(out, "Every entry in the streams this console reads was decoded.");
        }
        out.push_str("<table>");
        for u in &self.unreadable {
            out.push_str(&format!("<tr><td>{}<td>position {}</tr>", text(u.stream), u.position));
        }
        out.push_str("</table>");
    }

    fn signals(&self) -> Vec<Signal> {
        if self.unreadable.is_empty() {
            return Vec::new();
        }
        let value = self.unreadable.len().to_string();
        vec![Signal { label: "unreadable entries", value, level: Level::Alarm }]
    }

    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[("stream", Unit::Text), ("position", Unit::Count)]);
        for u in &self.unreadable {
            table.rows.push(vec![Cell::text(u.stream), Cell::Integer(i128::from(u.position))]);
        }
        Some(table)
    }
}
