// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a venue states about orders, fills and positions, normalised by its adapter. Evidence, not
//! commands: reconciliation reads it against the ledger and never takes it on trust.

use bijective_model::market::ExchangeTime;
use bijective_model::{Side, VenueSymbol};

use crate::fill::{Fee, Fill};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedStatus {
    Working,
    Filled,
    Canceled,
    Expired,
    Rejected,
}

/// One order as the venue holds it. `client_id` is absent when the venue holds an order this
/// platform did not identify, which makes it external.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderStatusReport {
    pub client_id: Option<String>,
    pub venue_order_id: String,
    pub instrument: VenueSymbol,
    pub side: Side,
    pub ordered_quantity_scaled: i128,
    pub filled_quantity_scaled: i128,
    pub status: ReportedStatus,
    pub reported_at: ExchangeTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillReport {
    pub client_id: Option<String>,
    pub instrument: VenueSymbol,
    pub fill: Fill,
}

/// An explicit position, signed: negative is short, zero is an explicit flat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionReport {
    pub instrument: VenueSymbol,
    pub quantity_scaled: i128,
}

/// A missing report is not a flat position, and a venue that publishes no positions has said
/// nothing about them. Both are kept distinct from an explicit zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionEvidence {
    Reported(Vec<PositionReport>),
    NotPublished,
}

/// A charge to the account that names no order, such as a venue's daily regulatory fee. The
/// fee's amount is positive when paid and negative when credited, as a fill's fee is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCharge {
    /// The venue's identifier, unique per charge, so a charge read twice is recorded once.
    pub charge_id: String,
    pub fee: Fee,
}

/// Everything one reconciliation pass read from a venue, with what it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueEvidence {
    pub orders: Vec<OrderStatusReport>,
    pub fills: Vec<FillReport>,
    /// Account charges read in the same window as the fills. Empty where the venue charges none
    /// apart from its fills.
    pub charges: Vec<AccountCharge>,
    pub positions: PositionEvidence,
    /// History before this instant, in UTC nanoseconds, was not read. `None`: no bound declared.
    pub history_from_ns: Option<i128>,
    /// Every source the adapter needed answered and every record mapped. False keeps the verdict
    /// from being `Aligned`, whatever else agrees.
    pub complete: bool,
}
