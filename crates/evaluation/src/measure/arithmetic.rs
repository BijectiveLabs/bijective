// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Integer statistics in basis points. Every rounding goes against the strategy.

/// Mean rounded down: for a signed figure that is the less favourable one.
pub(crate) fn floor_mean_of(values: &[i128]) -> i128 {
    if values.is_empty() {
        return 0;
    }
    let total: i128 = values.iter().sum();
    floor_div(total, values.len() as i128).unwrap_or(0)
}

pub(crate) fn floor_div(numerator: i128, denominator: i128) -> Option<i128> {
    if denominator == 0 {
        return None;
    }
    let q = numerator / denominator;
    if numerator % denominator != 0 && ((numerator < 0) != (denominator < 0)) {
        Some(q - 1)
    } else {
        Some(q)
    }
}

pub(crate) fn mean_of(values: &[i128]) -> i128 {
    if values.is_empty() {
        return 0;
    }
    let total: i128 = values.iter().sum();
    ceil_div(total, values.len() as i128).unwrap_or(0)
}

/// Population deviation, rounded up at every step because it sizes the evidence.
pub(crate) fn deviation_of(values: &[i128], mean: i128) -> i128 {
    if values.len() < 2 {
        return 0;
    }
    let mut total = 0i128;
    for v in values {
        let d = v - mean;
        total = total.saturating_add(d.saturating_mul(d));
    }
    let variance = ceil_div(total, values.len() as i128).unwrap_or(0);
    ceil_sqrt(variance)
}

pub(crate) fn ceil_div(numerator: i128, denominator: i128) -> Option<i128> {
    if denominator == 0 {
        return None;
    }
    let q = numerator / denominator;
    if numerator % denominator == 0 { Some(q) } else { Some(q + 1) }
}

pub(crate) fn ceil_sqrt(value: i128) -> i128 {
    if value <= 0 {
        return 0;
    }
    let mut low = 0i128;
    let mut high = 1i128;
    while high.saturating_mul(high) < value {
        high = high.saturating_mul(2);
    }
    while low < high {
        let mid = low + (high - low) / 2;
        if mid.saturating_mul(mid) < value {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}
