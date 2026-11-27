// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Risk limit evaluation core: decides whether declared limits are breached.
//! Pure computation (no clock, no I/O) so a decision can be reproduced when asked why it fired.

mod amount;
pub use amount::*;
mod scope;
pub use scope::*;
mod limit;
pub use limit::*;
mod safe_state;
pub use safe_state::*;
mod pre_trade;
pub use pre_trade::*;
mod provenance;
pub use provenance::*;

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
