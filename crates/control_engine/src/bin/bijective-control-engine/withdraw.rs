// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a pass withdraws and closes once the proposals are answered: resting orders the trading
//! state no longer admits, then the positions a flatten command asked closed (decision 0010).

use bijective_control_engine::flatten::flatten_held;
use bijective_control_engine::{OrderPath, Reachable, withdraw_resting};

use crate::clock::{clock, now_ns};
use crate::{Fatal, PROGRAM};

pub(crate) async fn withdraw_and_flatten(
    path: &mut OrderPath,
    reachable: &mut Reachable<'_>,
) -> Result<(), Fatal> {
    let swept = withdraw_resting(path, reachable, now_ns())
        .await
        .map_err(|e| Fatal::failure(format!("a withdrawal was not recorded: {e}")))?;
    if !swept.withdrawn.is_empty() || !swept.unreachable.is_empty() {
        eprintln!(
            "{PROGRAM}: {} resting order(s) withdrawn, {} on a venue that cannot be reached",
            swept.withdrawn.len(),
            swept.unreachable.len()
        );
    }
    // After the sweep, so the orders that could add exposure are already asked cancelled.
    let flattened = flatten_held(path, reachable, clock())
        .await
        .map_err(|e| Fatal::failure(format!("a flatten order was not recorded: {e}")))?;
    if !flattened.placed.is_empty() {
        eprintln!(
            "{PROGRAM}: flatten: {} order(s) submitted or refused; untrusted {:?}; unreachable {:?}",
            flattened.placed.len(),
            flattened.untrusted,
            flattened.unreachable
        );
    }
    Ok(())
}
