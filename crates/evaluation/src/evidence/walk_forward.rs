// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Walk-forward structure a promotion requires.
//! Windows are store positions, so every rule is an integer comparison.

use bijective_strategy_engine::runs::Window;

/// Half-open: `from` is read, `until` is not. An empty segment stays sayable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Positions {
    pub from: u64,
    pub until: u64,
}

impl Positions {
    #[must_use]
    pub const fn new(from: u64, until: u64) -> Self {
        Self { from, until }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.until <= self.from
    }

    #[must_use]
    pub const fn overlaps(&self, other: &Self) -> bool {
        self.from < other.until && other.from < self.until
    }
}

impl core::fmt::Display for Positions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[{}, {})", self.from, self.until)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    pub learn: Positions,
    pub test: Positions,
}

/// Each variant is reachable and tested. A check that cannot fire is worse than none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkForwardError {
    NoFolds,
    EmptySegment {
        fold: usize,
        segment: &'static str,
    },
    /// Look-ahead: the test is measured on history already learned from.
    TestReachesBackwards {
        fold: usize,
        learn: Positions,
        test: Positions,
    },
    /// Overlapping tests count one stretch twice and inflate the sample.
    TestsOverlap {
        earlier: usize,
        later: usize,
        at: Positions,
    },
    OutsideWindow {
        fold: usize,
        reaches: u64,
        window_ends: u64,
    },
}

impl core::fmt::Display for WalkForwardError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WalkForwardError::NoFolds => {
                write!(f, "no folds, so nothing was held out")
            }
            WalkForwardError::EmptySegment { fold, segment } => {
                write!(f, "fold {fold}'s {segment} segment reads nothing")
            }
            WalkForwardError::TestReachesBackwards { fold, learn, test } => write!(
                f,
                "fold {fold} tests {test} against a rule learned to {}, so the test saw history the rule already had",
                learn.until
            ),
            WalkForwardError::TestsOverlap { earlier, later, at } => write!(
                f,
                "folds {earlier} and {later} both test {at}, counting one stretch of history twice"
            ),
            WalkForwardError::OutsideWindow { fold, reaches, window_ends } => write!(
                f,
                "fold {fold} reaches position {reaches}, past the {window_ends} the run was allowed to read"
            ),
        }
    }
}

impl std::error::Error for WalkForwardError {}

/// A validated walk-forward. Construction is the validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkForward {
    folds: Vec<Fold>,
}

impl WalkForward {
    /// The window bounds every fold: testing past what the run could read measures events it never had.
    pub fn declare(folds: Vec<Fold>, window: Window) -> Result<Self, WalkForwardError> {
        if folds.is_empty() {
            return Err(WalkForwardError::NoFolds);
        }

        let bound = match window {
            Window::ToPosition(p) => Some(p),
            // Neither variant bounds a position.
            Window::ToPresent | Window::RecordOnly => None,
        };

        for (i, fold) in folds.iter().enumerate() {
            if fold.learn.is_empty() {
                return Err(WalkForwardError::EmptySegment { fold: i, segment: "learn" });
            }
            if fold.test.is_empty() {
                return Err(WalkForwardError::EmptySegment { fold: i, segment: "test" });
            }
            if fold.test.from < fold.learn.until {
                return Err(WalkForwardError::TestReachesBackwards {
                    fold: i,
                    learn: fold.learn,
                    test: fold.test,
                });
            }
            if let Some(end) = bound {
                let reaches = fold.test.until.max(fold.learn.until);
                // The window is inclusive and a fold is half-open, so compare against end + 1.
                // Comparing against end made the last readable position untestable.
                if reaches > end.saturating_add(1) {
                    return Err(WalkForwardError::OutsideWindow {
                        fold: i,
                        reaches,
                        window_ends: end,
                    });
                }
            }
        }

        // Tests only. Learn segments overlap by design.
        for (i, a) in folds.iter().enumerate() {
            for (j, b) in folds.iter().enumerate().skip(i + 1) {
                if a.test.overlaps(&b.test) {
                    return Err(WalkForwardError::TestsOverlap {
                        earlier: i,
                        later: j,
                        at: b.test,
                    });
                }
            }
        }

        Ok(Self { folds })
    }

    /// Not public: only the registry rebuilds from the stream, and the original window is not in the record.
    pub(crate) fn as_read(folds: Vec<Fold>) -> Self {
        Self { folds }
    }

    #[must_use]
    pub fn folds(&self) -> &[Fold] {
        &self.folds
    }

    /// Held-out total only. Counting fitted positions would flatter the result.
    #[must_use]
    pub fn held_out_positions(&self) -> u64 {
        self.folds
            .iter()
            .map(|f| f.test.until.saturating_sub(f.test.from))
            .fold(0u64, u64::saturating_add)
    }
}
