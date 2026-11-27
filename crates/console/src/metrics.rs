// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The strip as OpenMetrics text, for a scraper that raises alerts where no operator is watching.
//!
//! The same signals the strip shows, one gauge per label at the highest level any panel gave it:
//! a halt, a divergence, an unread evidence or an unmeasured limit reaches an alert rule exactly
//! as it reaches the page. Delivery (mail, pager, chat) is the scraper's alert manager's. The
//! console sends nothing.

use std::collections::BTreeMap;

use crate::panel::{Level, Signal};

/// The media type the text is served as.
pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

fn level_value(level: Level) -> u8 {
    match level {
        Level::Normal => 0,
        Level::Attention => 1,
        Level::Alarm => 2,
    }
}

/// A label value with the three characters the format escapes.
fn escaped(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// The exposition for `signals`. Two signals under one label are one series, at the higher level,
/// so the text never carries a duplicate series.
#[must_use]
pub fn render(signals: &[Signal]) -> String {
    let mut by_label: BTreeMap<&str, u8> = BTreeMap::new();
    for signal in signals {
        let level = level_value(signal.level);
        let held = by_label.entry(signal.label).or_insert(level);
        *held = (*held).max(level);
    }
    let alarms = by_label.values().filter(|l| **l == 2).count();
    let mut out = String::new();
    out.push_str("# TYPE bijective_signal gauge\n");
    out.push_str(
        "# HELP bijective_signal A figure on the console's strip: 0 normal, 1 attention, 2 alarm.\n",
    );
    for (label, level) in &by_label {
        out.push_str(&format!("bijective_signal{{label=\"{}\"}} {level}\n", escaped(label)));
    }
    out.push_str("# TYPE bijective_alarms gauge\n");
    out.push_str("# HELP bijective_alarms Strip figures at alarm.\n");
    out.push_str(&format!("bijective_alarms {alarms}\n"));
    out.push_str("# EOF\n");
    out
}

#[cfg(test)]
#[path = "../tests/unit/metrics_tests.rs"]
mod tests;
