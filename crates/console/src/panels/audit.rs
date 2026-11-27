// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Operator commands as the control engine carried them out or refused them.

use bijective_authority::{Action, Role};
use bijective_control_engine::wire::ControlEvent;
use bijective_limits::SafeState;

use super::keep;
use crate::markup::{nothing, table_head, text};
use crate::panel::{Entry, Panel};
use crate::table::{Cell, Table, Unit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRow {
    pub command_id: String,
    pub actor: String,
    pub role: Role,
    pub action: Action,
    pub state_after: SafeState,
    /// Empty where it was carried out. Anything else is why it was not.
    pub detail: String,
    pub at_ns: i128,
}

#[derive(Default)]
pub struct ActionsPanel {
    pub actions: Vec<ActionRow>,
}

impl Panel for ActionsPanel {
    fn title(&self) -> &'static str {
        "Operator actions"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        let Entry::Control(ControlEvent::CommandApplied {
            command_id,
            actor,
            role,
            action,
            state,
            detail,
            at_ns,
            ..
        }) = entry
        else {
            return;
        };
        let row = ActionRow {
            command_id: command_id.clone(),
            actor: actor.clone(),
            role: *role,
            action: *action,
            state_after: *state,
            detail: detail.clone(),
            at_ns: *at_ns,
        };
        keep(&mut self.actions, row);
    }

    fn render(&self, out: &mut String) {
        if self.actions.is_empty() {
            return nothing(out, "No command has been carried out.");
        }
        table_head(out, &["command", "actor", "role", "action", "state after", "note", "at"]);
        for a in &self.actions {
            let note = if a.detail.is_empty() {
                "carried out".to_string()
            } else {
                format!("<strong>{}</strong>", text(&a.detail))
            };
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}<td>{}<td>{:?}<td>{note}<td>{}</tr>",
                text(&a.command_id),
                text(&a.actor),
                text(a.role.as_str()),
                text(a.action.as_str()),
                a.state_after,
                a.at_ns
            ));
        }
        out.push_str("</table>");
    }

    /// `refusal` is absent where the command was carried out, and says why where it was not.
    fn rows(&self) -> Option<Table> {
        let mut table = Table::new(&[
            ("command", Unit::Text),
            ("actor", Unit::Text),
            ("role", Unit::Text),
            ("action", Unit::Text),
            ("state after", Unit::Text),
            ("refusal", Unit::Text),
            ("at", Unit::Nanoseconds),
        ]);
        for a in &self.actions {
            let refusal = (!a.detail.is_empty()).then_some(a.detail.as_str());
            table.rows.push(vec![
                Cell::text(a.command_id.clone()),
                Cell::text(a.actor.clone()),
                Cell::text(a.role.as_str()),
                Cell::text(a.action.as_str()),
                Cell::text(format!("{:?}", a.state_after).to_lowercase()),
                Cell::optional_text(refusal),
                Cell::Integer(a.at_ns),
            ]);
        }
        Some(table)
    }
}
