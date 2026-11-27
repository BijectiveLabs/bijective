// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Whether a research verdict says anything.
//! Design: research integrity,.

pub mod preregistration;
pub mod registry;
pub mod walk_forward;

pub use preregistration::{
    DecisionRule, Direction, Horizon, Measured, Outcome, PreRegistration, RegistrationError,
    Unregistered, Verdict, judge,
};
pub use walk_forward::{Fold, Positions, WalkForward, WalkForwardError};

#[cfg(test)]
#[path = "../../tests/unit/evidence/tests.rs"]
mod tests;

/// Same scale as risk limits. Restated so this crate does not depend on limits.
pub const ONE: i128 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceError {
    Overflow,
    /// Zero effect is never distinguishable from noise.
    EffectIsZero,
    NotPositive,
}

/// Three answers, not two: an underpowered result is neither a rejection nor a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    Sufficient {
        required: u64,
        observed: u64,
    },
    Insufficient {
        required: u64,
        observed: u64,
        short_by: u64,
    },
    /// Not a pass: an evidence requirement that cannot be sized is unsupported.
    Unknowable {
        cause: EvidenceError,
    },
}

impl Evidence {
    /// Only `Sufficient` supports promotion. The other two mean the platform does not know.
    #[must_use]
    pub fn supports_promotion(self) -> bool {
        matches!(self, Evidence::Sufficient { .. })
    }
}

impl core::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EvidenceError::Overflow => write!(f, "the requirement does not fit"),
            EvidenceError::EffectIsZero => {
                write!(f, "an effect of zero is not distinguishable at any sample size")
            }
            EvidenceError::NotPositive => write!(f, "dispersion and confidence must be positive"),
        }
    }
}

impl core::fmt::Display for Evidence {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Evidence::Sufficient { required, observed } => {
                write!(f, "sufficient: {observed} observation(s) against {required} required")
            }
            Evidence::Insufficient { required, observed, short_by } => write!(
                f,
                "insufficient: {observed} observation(s) against {required} required, short by {short_by}"
            ),
            Evidence::Unknowable { cause } => write!(f, "unknowable: {cause}"),
        }
    }
}

/// `confidence` is the operator's declared critical value in the fixed scale, never defaulted.
/// Divisions round towards requiring more evidence.
pub fn observations_required(
    confidence: i128,
    dispersion: i128,
    effect: i128,
) -> Result<u64, EvidenceError> {
    if confidence <= 0 || dispersion <= 0 {
        return Err(EvidenceError::NotPositive);
    }
    if effect == 0 {
        return Err(EvidenceError::EffectIsZero);
    }

    let effect = effect.checked_abs().ok_or(EvidenceError::Overflow)?;

    let noise_to_effect =
        ceil_div(dispersion.checked_mul(ONE).ok_or(EvidenceError::Overflow)?, effect)?;
    let quotient =
        ceil_div(confidence.checked_mul(noise_to_effect).ok_or(EvidenceError::Overflow)?, ONE)?;

    let squared = quotient.checked_mul(quotient).ok_or(EvidenceError::Overflow)?;
    let required = ceil_div(squared, ONE.checked_mul(ONE).ok_or(EvidenceError::Overflow)?)?;

    u64::try_from(required).map_err(|_| EvidenceError::Overflow)
}

pub fn assess(observed: u64, confidence: i128, dispersion: i128, effect: i128) -> Evidence {
    match observations_required(confidence, dispersion, effect) {
        Err(cause) => Evidence::Unknowable { cause },
        Ok(required) if observed >= required => Evidence::Sufficient { required, observed },
        Ok(required) => {
            Evidence::Insufficient { required, observed, short_by: required - observed }
        }
    }
}

/// Rounds away from zero. Callers have already checked positive inputs.
fn ceil_div(numerator: i128, denominator: i128) -> Result<i128, EvidenceError> {
    if denominator <= 0 {
        return Err(EvidenceError::NotPositive);
    }
    let quotient = numerator.checked_div(denominator).ok_or(EvidenceError::Overflow)?;
    if numerator % denominator == 0 {
        Ok(quotient)
    } else {
        quotient.checked_add(1).ok_or(EvidenceError::Overflow)
    }
}

/// Whether a verdict may be promoted. The in-sample figure is kept for comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Promotable {
        held_out_positions: u64,
        evidence: Evidence,
    },
    /// No held-out structure. Describes the fitted sample only.
    InSampleOnly {
        evidence: Evidence,
    },
    Underpowered {
        evidence: Evidence,
    },
}

impl Standing {
    #[must_use]
    pub fn may_promote(self) -> bool {
        matches!(self, Standing::Promotable { .. })
    }
}

impl core::fmt::Display for Standing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Standing::Promotable { held_out_positions, evidence } => {
                write!(f, "promotable on {held_out_positions} held-out position(s); {evidence}")
            }
            Standing::InSampleOnly { evidence } => {
                write!(f, "in-sample only, so not promotable on its own evidence; {evidence}")
            }
            Standing::Underpowered { evidence } => {
                write!(f, "held out, and not promotable; {evidence}")
            }
        }
    }
}

/// Promotable only when held out and the evidence is sufficient..
#[must_use]
pub fn stand(held_out: Option<&WalkForward>, evidence: Evidence) -> Standing {
    match held_out {
        None => Standing::InSampleOnly { evidence },
        Some(_) if !evidence.supports_promotion() => Standing::Underpowered { evidence },
        Some(w) => Standing::Promotable { held_out_positions: w.held_out_positions(), evidence },
    }
}
