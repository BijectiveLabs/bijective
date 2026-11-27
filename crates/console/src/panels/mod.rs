// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Every panel of the console, and the groups the navigation shows them in.
//!
//! This is the one list. A new panel is written in its own file and named once in `groups`.

mod audit;
mod control;
mod data;
mod detections;
mod execution;
mod exposure;
mod facts;
mod fills;
mod held;
mod latency;
mod market;
mod orders;
mod risk;
mod scales;
mod scopes;
mod standing;
mod strategies;
mod venues;

pub use audit::ActionsPanel;
pub use control::{ControlPanel, Offer};
pub use data::{LinksPanel, UnreadablePanel};
pub use detections::{DetectionsPanel, StandingDetectionsPanel};
pub use execution::{BPS_SCALE, Costs, ExecutionPanel, costs as arrival_costs};
pub use exposure::ExposurePanel;
pub use facts::{LiquidationsPanel, ReportedPanel};
pub use fills::{ChargesPanel, FillsPanel};
pub use held::HeldPanel;
pub use latency::LatencyPanel;
pub use market::MarketPanel;
pub use orders::{DecisionsPanel, OrdersPanel, RefusedPanel};
pub use risk::{LimitsPanel, PostconditionsPanel, SafeStatePanel};
pub use scopes::ScopesPanel;
pub use standing::{Standing, StandingPanel};
pub use strategies::{StrategiesPanel, StrategyHistoryPanel};
pub use venues::{ReconciliationPanel, VenuesPanel};

use crate::panel::Panel;

/// Rows kept per history. Counts come from the whole fold. Rows are the tail, so a long-running
/// node stays bounded.
pub const TAIL: usize = 200;

/// Panels shown together under one route.
pub struct Group {
    /// The path segment, without a slash. The overview is the empty route.
    pub route: &'static str,
    pub label: &'static str,
    pub panels: Vec<Box<dyn Panel>>,
}

/// The console's groups, in navigation order.
#[must_use]
pub fn groups() -> Vec<Group> {
    vec![
        group("", "Overview", vec![Box::new(StandingPanel::default())]),
        group(
            "market",
            "Market",
            vec![
                Box::new(MarketPanel::default()),
                Box::new(ReportedPanel::default()),
                Box::new(LiquidationsPanel::default()),
            ],
        ),
        group(
            "data",
            "Data",
            vec![Box::new(LinksPanel::default()), Box::new(UnreadablePanel::default())],
        ),
        group(
            "venues",
            "Venues",
            vec![
                Box::new(VenuesPanel::default()),
                Box::new(LatencyPanel::default()),
                Box::new(ReconciliationPanel::default()),
            ],
        ),
        group(
            "orders",
            "Orders",
            vec![
                Box::new(OrdersPanel::default()),
                Box::new(FillsPanel::default()),
                Box::new(ExecutionPanel::default()),
                Box::new(HeldPanel::default()),
                Box::new(RefusedPanel::default()),
                Box::new(DecisionsPanel::default()),
            ],
        ),
        group(
            "risk",
            "Risk",
            vec![
                Box::new(ExposurePanel::default()),
                Box::new(ScopesPanel::default()),
                Box::new(ChargesPanel::default()),
                Box::new(LimitsPanel::default()),
                Box::new(SafeStatePanel::default()),
                Box::new(PostconditionsPanel::default()),
            ],
        ),
        group(
            "strategies",
            "Strategies",
            vec![Box::new(StrategiesPanel::default()), Box::new(StrategyHistoryPanel::default())],
        ),
        group(
            "detections",
            "Detections",
            vec![
                Box::new(StandingDetectionsPanel::default()),
                Box::new(DetectionsPanel::default()),
            ],
        ),
        group("audit", "Audit", vec![Box::new(ActionsPanel::default())]),
        // The state beside the forms, so an operator sees what a command would act on.
        group(
            "control",
            "Control",
            vec![Box::new(SafeStatePanel::default()), Box::new(ControlPanel::default())],
        ),
    ]
}

fn group(route: &'static str, label: &'static str, panels: Vec<Box<dyn Panel>>) -> Group {
    Group { route, label, panels }
}

pub(crate) fn keep<T>(rows: &mut Vec<T>, row: T) {
    rows.push(row);
    if rows.len() > TAIL {
        rows.remove(0);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/panels_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/panel_tables_tests.rs"]
mod table_tests;
