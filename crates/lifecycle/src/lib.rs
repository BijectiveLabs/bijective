// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Order lifecycle, fills and positions: the execution domain as pure state machines.
//!
//! An order moves only by an applied event. A position moves only by a fill or an audited
//! adjustment. Each change yields one audit record, and each refusal leaves state as it was.
//! Nothing here reads a clock, opens a file or knows a venue, so live, replay and reconciliation
//! fold the same events to the same state. Design notes: `docs/concepts/order_lifecycle.md`.

pub mod fill;
pub mod ledger;
pub mod order;
pub mod position;
pub mod reconcile;
pub mod report;

pub use fill::{Fee, Fill, Liquidity};
pub use ledger::{Ledger, LedgerChange, LedgerError, LedgerSummary};
pub use order::{Applied, LifecycleError, Order, OrderEvent, OrderState, Transition};
pub use position::{Lot, Position, PositionCause, PositionChange, PositionError};
pub use reconcile::{Finding, Reconciliation, Verdict, reconcile};
pub use report::{
    AccountCharge, FillReport, OrderStatusReport, PositionEvidence, PositionReport, ReportedStatus,
    VenueEvidence,
};

#[cfg(test)]
#[path = "../tests/unit/ledger_tests.rs"]
mod ledger_tests;
#[cfg(test)]
#[path = "../tests/unit/order_tests.rs"]
mod order_tests;
#[cfg(test)]
#[path = "../tests/unit/position_tests.rs"]
mod position_tests;
#[cfg(test)]
#[path = "../tests/unit/reconcile_tests.rs"]
mod reconcile_tests;
