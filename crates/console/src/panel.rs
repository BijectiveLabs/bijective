// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The one contract every panel of the console implements.
//!
//! A panel owns its fold and its view. The console reads each stream once, decodes each entry
//! once, and offers it to every panel. A panel keeps what it needs and ignores the rest. Adding a
//! panel is one file and one line in `panels::groups`, and touches no other panel.

use bijective_archive::FactRow;
use bijective_control_engine::wire::ControlEvent;
use bijective_data_engine::link::LinkTransition;
use bijective_risk::limit_state::wire::LimitEvent;
use bijective_serialization::signal::market::BookUpdate;

use crate::desk::Desk;
use crate::panels::Standing;
use crate::table::Table;

/// A store entry this build could not decode. Listed, never skipped: a quietly different view
/// hides the difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub stream: &'static str,
    pub position: u64,
}

/// One decoded entry, as the console offers it to the panels.
pub enum Entry<'a> {
    /// The node's standing as the deployment kit answers it now. Offered first on every request.
    Standing(&'a Standing),
    /// Who this console signs commands as, offered first on every request when it was given an
    /// operator. Absent, the console issues nothing.
    Desk(&'a Desk),
    Control(&'a ControlEvent),
    Limit(&'a LimitEvent),
    /// A transition on one venue's link stream, named after the stream it came from.
    Link {
        venue: &'a str,
        transition: &'a LinkTransition,
    },
    /// A top of book from one venue's market stream. Only the newest are offered.
    Book {
        venue: &'a str,
        update: &'a BookUpdate,
    },
    /// A trade, liquidation, funding, open interest, mark or index row from the newest files of
    /// one venue's market archive, with the scales its file declares. Offered in journal order.
    Fact {
        venue: &'a str,
        price_scale: u8,
        quantity_scale: u8,
        row: &'a FactRow,
    },
    /// A conclusion some program wrote to the detections stream (`crate::detection`).
    Detection(&'a crate::detection::Detection),
    /// What some program concluded about a strategy's orders (`crate::performance`), in the run it
    /// was read from: a run mode of this store, or `backtest <window>` for a backtest record.
    Performance {
        run: &'a str,
        performance: &'a crate::performance::Performance,
    },
    Unreadable(&'a Unreadable),
}

/// How much a figure on the top strip asks of an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Normal,
    Attention,
    Alarm,
}

/// One figure a panel contributes to the strip that is on every page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub label: &'static str,
    /// Plain text. The strip escapes it.
    pub value: String,
    pub level: Level,
}

pub trait Panel {
    /// The heading the panel renders under.
    fn title(&self) -> &'static str;

    /// Called once per entry, in stream order. Must not fail: what a panel cannot use, it skips,
    /// and what nobody can decode arrives as `Entry::Unreadable`.
    fn apply(&mut self, entry: &Entry<'_>);

    /// Writes the panel's body. Text that did not come from this build goes through
    /// `markup::text`.
    fn render(&self, out: &mut String);

    /// What this panel puts on the top strip. Most panels put nothing there.
    fn signals(&self) -> Vec<Signal> {
        Vec::new()
    }

    /// The panel's figures as a table for programs (`table`), from the same fold as `render`.
    /// `None` for a panel shown as a page only.
    fn rows(&self) -> Option<Table> {
        None
    }
}
