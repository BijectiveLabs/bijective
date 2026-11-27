// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The program clock: chosen once from the run plan, before any time read, and read through here.

use bijective_common::clock::{Clock, SequenceClock};
use bijective_control_engine::{Inputs, RunPlan};

static CLOCK: std::sync::OnceLock<Box<dyn Clock>> = std::sync::OnceLock::new();

/// A backtest's clock, which its recording moves (decision 0015). Unset in every other run.
static RECORDED: std::sync::OnceLock<&'static SequenceClock> = std::sync::OnceLock::new();

/// The recorded clock as the program reads it.
struct Recorded(&'static SequenceClock);

impl Clock for Recorded {
    fn now_ns(&self) -> i128 {
        self.0.now_ns()
    }
}

/// One clock per program, anchored at start. All time reads go through it.
pub(crate) fn clock() -> &'static dyn Clock {
    CLOCK.get_or_init(|| Box::new(bijective_common::clock::WallClock::new())).as_ref()
}

pub(crate) fn now_ns() -> i128 {
    clock().now_ns()
}

/// Fixes the program clock. Refused if a clock was already read, so a backtest cannot silently
/// run on the host clock.
pub(crate) fn fix(plan: RunPlan) -> Result<(), &'static str> {
    let chosen: Box<dyn Clock> = match plan.inputs() {
        Inputs::Recorded { from_ns } => {
            // Lives for the process, as the clock it backs does.
            let recorded: &'static SequenceClock =
                Box::leak(Box::new(SequenceClock::starting_at(from_ns)));
            RECORDED.set(recorded).map_err(|_| "the recorded clock was set twice")?;
            Box::new(Recorded(recorded))
        }
        Inputs::Live => plan.clock(),
    };
    CLOCK.set(chosen).map_err(|_| "the clock was read before it was chosen")
}

/// Moves a backtest's clock to `at_ns`. An earlier instant leaves it where it is: the clock
/// never moves back.
pub(crate) fn advance_to(at_ns: i128) -> Result<(), String> {
    let Some(recorded) = RECORDED.get() else {
        return Err("only a backtest's clock is moved by its recording".into());
    };
    if at_ns <= recorded.now_ns() {
        return Ok(());
    }
    recorded.advance_to(at_ns).map_err(|e| format!("the recorded clock refused {at_ns}: {e:?}"))
}
