// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Open-divergence signal for the console.

use crate::classify::{Divergence, DivergenceClass};

/// How loud an open divergence should be. Driven by age, not count: a count reads the same on day one and day thirty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Raised,
    Ageing,
    Overdue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDivergence {
    pub divergence: Divergence,
    pub age_ns: i128,
    pub severity: Severity,
}

/// One hour, then one day. Kept here so two readers cannot disagree on urgency.
/// same divergence cannot disagree about how urgent it is.
const AGEING_AFTER_NS: i128 = 3_600_000_000_000;
const OVERDUE_AFTER_NS: i128 = 86_400_000_000_000;

pub fn open_signal(open: &[Divergence], now_ns: i128) -> Vec<OpenDivergence> {
    let mut out: Vec<OpenDivergence> = open
        .iter()
        .map(|d| {
            let age_ns = (now_ns - d.raised_at_ns).max(0);
            OpenDivergence { divergence: d.clone(), age_ns, severity: severity_for(d, age_ns) }
        })
        .collect();

    // Oldest first, so the longest-open item is not buried under newer ones.
    out.sort_by_key(|d| std::cmp::Reverse(d.age_ns));
    out
}

fn severity_for(divergence: &Divergence, age_ns: i128) -> Severity {
    // An Extra starts loud: waiting an hour would make severity a function of patience.
    if divergence.class == DivergenceClass::Extra {
        return Severity::Overdue;
    }
    if age_ns >= OVERDUE_AFTER_NS {
        Severity::Overdue
    } else if age_ns >= AGEING_AFTER_NS {
        Severity::Ageing
    } else {
        Severity::Raised
    }
}
