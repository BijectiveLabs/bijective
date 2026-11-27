// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The control page: forms that ask the console to sign a command as its operator.
//!
//! Every form posts to `/control` with the console's token. Which actions appear is the
//! operator's role, decided by the access table. The control engine decides again on arrival.

use std::collections::BTreeSet;

use bijective_authority::{Action, Role};

use crate::markup::{nothing, text};
use crate::panel::{Entry, Panel};

/// What the desk let this page offer, as of the latest request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub operator: String,
    pub role: Role,
    pub actions: Vec<Action>,
    pub token: String,
}

#[derive(Default)]
pub struct ControlPanel {
    pub offer: Option<Offer>,
    /// Venues seen on a link stream, so a venue action names a venue the node knows.
    pub venues: BTreeSet<String>,
}

fn names_a_venue(action: Action) -> bool {
    matches!(action, Action::WithdrawVenue | Action::AdmitVenue)
}

impl Panel for ControlPanel {
    fn title(&self) -> &'static str {
        "Control"
    }

    fn apply(&mut self, entry: &Entry<'_>) {
        match entry {
            Entry::Desk(desk) => {
                self.offer = Some(Offer {
                    operator: desk.operator.clone(),
                    role: desk.role,
                    actions: desk.offers(),
                    token: desk.token().to_string(),
                });
            }
            Entry::Link { venue, .. } => {
                self.venues.insert((*venue).to_string());
            }
            _ => {}
        }
    }

    fn render(&self, out: &mut String) {
        let Some(offer) = &self.offer else {
            return nothing(
                out,
                "This console was not given an operator, so it issues nothing. Commands are \
                 issued with `bijective issue`, or by starting the console with --operator, \
                 --role and --key.",
            );
        };
        out.push_str(&format!(
            "<p>Signing as <strong>{}</strong> ({}). The control engine verifies the signature \
             and the role; what came of each command is under Audit.</p>",
            text(&offer.operator),
            offer.role.as_str()
        ));
        for action in &offer.actions {
            form(out, *action, &offer.token, &self.venues);
        }
    }
}

fn form(out: &mut String, action: Action, token: &str, venues: &BTreeSet<String>) {
    if names_a_venue(action) && venues.is_empty() {
        return;
    }
    out.push_str(&format!(
        "<form method=\"post\" action=\"/control\"><input type=\"hidden\" name=\"token\" \
         value=\"{}\"><input type=\"hidden\" name=\"action\" value=\"{}\">",
        text(token),
        action.as_str()
    ));
    if names_a_venue(action) {
        out.push_str("<select name=\"venue\">");
        for venue in venues {
            let venue = text(venue);
            out.push_str(&format!("<option value=\"{venue}\">{venue}</option>"));
        }
        out.push_str("</select> ");
    }
    out.push_str(&format!("<button type=\"submit\">{}</button></form>", action.as_str()));
}
