// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The panel keeps the newest entry per subject and answers it as written.

use super::*;
use crate::performance::{Markout, Performance};

fn entry(net: &str, at_ns: i128) -> Performance {
    Performance {
        desk: "alpha".into(),
        strategy: Some("trend".into()),
        venue: "venue-a".into(),
        instrument: "AAA".into(),
        identity: Some("venue-a.AAA".into()),
        quote_currency: Some("usd".into()),
        fills: 2,
        wins: 0,
        losses: 1,
        position: "0".into(),
        traded_notional: "200.00".into(),
        realized: "-0.40".into(),
        unrealized: Some("0".into()),
        fees: "0.10".into(),
        fees_excluded: Vec::new(),
        net: Some(net.into()),
        peak_net: Some("0".into()),
        drawdown: Some("0.50".into()),
        arrival_cost: Some("0.02".into()),
        arrival_cost_bps: Some("1.00".into()),
        markouts: MARKOUT_HORIZONS_S
            .iter()
            .map(|h| Markout { horizon_s: *h, bps: Some(format!("-{h}.00")), fills: 2 })
            .collect(),
        source: "control".into(),
        through: 9,
        at_ns,
    }
}

#[test]
fn the_newest_entry_per_subject_is_the_one_shown() {
    let mut panel = StrategiesPanel::default();
    for e in [entry("-0.30", 1), entry("-0.50", 2)] {
        panel.apply(&Entry::Performance { run: "paper", performance: &e });
    }
    let table = panel.rows().expect("a table");
    assert_eq!(table.rows.len(), 1);
    let row = &table.rows[0];
    assert_eq!(row.len(), COLUMNS.len());
    assert_eq!(row[0], Cell::Integer(2));
    assert_eq!(row[1], Cell::text("paper"));
    assert_eq!(row[5], Cell::text("venue-a.AAA"));
    assert_eq!(row[16], Cell::text("-0.50"));
}

/// The markout columns are the horizons the format names, in order.
#[test]
fn the_markout_columns_are_the_format_s_horizons() {
    let row = cells("live", &entry("0", 1));
    for (i, h) in MARKOUT_HORIZONS_S.iter().enumerate() {
        assert_eq!(COLUMNS[21 + i].0, format!("markout {h}s bps"));
        assert_eq!(row[21 + i], Cell::text(format!("-{h}.00")));
    }
    assert_eq!(row[26], Cell::Integer(2));
}

#[test]
fn the_history_keeps_every_entry_in_order() {
    let mut panel = StrategyHistoryPanel::default();
    for e in [entry("-0.30", 1), entry("-0.50", 2)] {
        panel.apply(&Entry::Performance { run: "paper", performance: &e });
    }
    let table = panel.rows().expect("a table");
    let nets: Vec<&Cell> = table.rows.iter().map(|r| &r[16]).collect();
    assert_eq!(nets, [&Cell::text("-0.30"), &Cell::text("-0.50")]);
}

/// The same strategy in two runs is two rows, side by side, never one overwriting the other.
#[test]
fn each_run_of_a_strategy_is_its_own_row() {
    let mut panel = StrategiesPanel::default();
    let (live, paper) = (entry("1.00", 1), entry("2.00", 2));
    panel.apply(&Entry::Performance { run: "live", performance: &live });
    panel.apply(&Entry::Performance { run: "paper", performance: &paper });
    let table = panel.rows().expect("a table");
    let modes: Vec<&Cell> = table.rows.iter().map(|r| &r[1]).collect();
    assert_eq!(modes, [&Cell::text("live"), &Cell::text("paper")]);
}
