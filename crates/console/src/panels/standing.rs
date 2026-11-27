// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Whether the node is up, as the deployment kit answers it.

use std::path::Path;

use bijective_deployment::manifest::Deployment;
use bijective_deployment::sequencer::{Node, NodeState, assess_node};

use crate::markup::text;
use crate::panel::{Entry, Level, Panel, Signal};
use crate::table::{Cell, Table, Unit};

/// Where the answer came from. The deployment kit already answers it. A second reduction could
/// disagree mid-incident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// The console was not told where the manifest and run directory are.
    NotConfigured,
    /// Named, never rendered as down: an unreadable manifest is not a node that is down.
    ManifestUnreadable {
        detail: String,
    },
    Assessed(Box<Node>),
}

impl Standing {
    /// The deployment kit's view of the node. `now_ns` is passed in so staleness can be driven
    /// through a stopped-reporting case.
    #[must_use]
    pub fn read(manifest: &Path, run_dir: &Path, now_ns: i128) -> Self {
        match std::fs::read(manifest)
            .map_err(|e| e.to_string())
            .and_then(|raw| Deployment::parse(&raw).map_err(|e| format!("{e:?}")))
        {
            Ok(deployment) => Self::Assessed(Box::new(assess_node(&deployment, run_dir, now_ns))),
            Err(detail) => Self::ManifestUnreadable { detail },
        }
    }
}

/// Holds the standing of the latest request only: it is asked of the deployment kit each time,
/// never folded.
pub struct StandingPanel {
    pub standing: Standing,
}

impl Default for StandingPanel {
    fn default() -> Self {
        Self { standing: Standing::NotConfigured }
    }
}

impl Panel for StandingPanel {
    fn title(&self) -> &'static str {
        "Standing"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        if let Entry::Standing(standing) = entry {
            self.standing = (*standing).clone();
        }
    }

    fn render(&self, out: &mut String) {
        match &self.standing {
            Standing::NotConfigured => out.push_str(
                "<p class=\"none\">This console was not told where the manifest and run directory \
                 are, so it cannot say whether the node is up.</p>",
            ),
            Standing::ManifestUnreadable { detail } => out.push_str(&format!(
                "<p><strong>The manifest could not be read</strong> - {}. This is not the same as \
                 the node being down.</p>",
                text(detail)
            )),
            Standing::Assessed(node) => programs(out, node),
        }
    }

    fn signals(&self) -> Vec<Signal> {
        let (value, level) = match &self.standing {
            Standing::NotConfigured => ("not configured".to_string(), Level::Attention),
            Standing::ManifestUnreadable { .. } => {
                ("manifest unreadable".to_string(), Level::Alarm)
            }
            Standing::Assessed(node) => (
                format!("{:?}", node.state),
                match node.state {
                    NodeState::Ready => Level::Normal,
                    NodeState::Starting => Level::Attention,
                    NodeState::Down | NodeState::Failed => Level::Alarm,
                },
            ),
        };
        vec![Signal { label: "node", value, level }]
    }

    /// One row per program the manifest names. None while the node is not assessed, which the
    /// strip's `node` figure says.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(PROGRAM_COLUMNS);
        if let Standing::Assessed(node) = &self.standing {
            for p in &node.programs {
                let health = match (p.health, p.lapsed) {
                    (None, false) => "never reported".to_string(),
                    (_, true) => "stopped reporting".to_string(),
                    (Some(h), false) => format!("{h:?}").to_lowercase(),
                };
                table.rows.push(vec![
                    Cell::text(p.name.clone()),
                    Cell::text(format!("{:?}", p.class).to_lowercase()),
                    Cell::text(health),
                    Cell::text(p.detail.clone()),
                ]);
            }
        }
        Some(table)
    }
}

const PROGRAM_COLUMNS: &[(&str, Unit)] =
    &[("program", Unit::Text), ("class", Unit::Text), ("health", Unit::Text), ("note", Unit::Text)];

/// Per-program health, and the operational programs reported degraded.
fn programs(out: &mut String, node: &Node) {
    out.push_str(&format!("<p>{:?} - {}</p>", node.state, text(&node.because)));
    crate::markup::table_head(out, &["program", "class", "health", "note"]);
    for p in &node.programs {
        let health = match (p.health, p.lapsed) {
            // Never reported and stopped reporting send an operator to different places.
            (None, false) => "<em>never reported</em>".to_string(),
            (_, true) => "<strong>stopped reporting</strong>".to_string(),
            (Some(h), false) => format!("{h:?}"),
        };
        out.push_str(&format!(
            "<tr><td>{}<td>{:?}<td>{health}<td>{}</tr>",
            text(&p.name),
            p.class,
            text(&p.detail)
        ));
    }
    out.push_str("</table>");

    let degraded = node.degradations();
    if !degraded.is_empty() {
        out.push_str("<p><strong>Operational programs degraded:</strong> ");
        out.push_str(&degraded.iter().map(|p| text(&p.name)).collect::<Vec<_>>().join(", "));
        out.push_str(" - the core runs without them; they are listed until they report again.</p>");
    }
}
