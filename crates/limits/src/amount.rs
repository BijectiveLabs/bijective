// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Fixed-point amounts, units, ratios and thresholds. Overflow is refused, never wrapped.

impl core::fmt::Display for Threshold {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // Scale is named, not applied: dividing would hide the low digits.
            Threshold::Money(amount) => {
                write!(f, "{} at scale {}", amount.scaled, Amount::SCALE)
            }
            Threshold::Ratio(ratio) => {
                write!(f, "{} of {}", ratio.scaled(), Ratio::ONE)
            }
        }
    }
}

impl core::fmt::Display for Amount {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} at scale {}", self.scaled, Self::SCALE)
    }
}

/// Refusal is a first-class outcome: a figure that cannot be computed must say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithmeticError {
    Overflow,
}

/// Integer in the platform's fixed scale, never a float. Overflow is refused, never wrapped (`docs/concepts/risk_and_limits.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount {
    /// Private so no caller can build an unscaled value by accident.
    pub(crate) scaled: i128,
}

impl Amount {
    /// Decimal places every `Amount` carries. Callers must not pick their own scale (`docs/concepts/risk_and_limits.md`).
    pub const SCALE: u8 = 8;

    /// Takes a value already in platform scale, so a raw unit count cannot be passed by mistake.
    #[must_use]
    pub const fn from_scaled(scaled: i128) -> Self {
        Self { scaled }
    }

    pub const fn add(self, other: Self) -> Result<Self, ArithmeticError> {
        match self.scaled.checked_add(other.scaled) {
            Some(sum) => Ok(Self { scaled: sum }),
            None => Err(ArithmeticError::Overflow),
        }
    }

    /// A wrapped subtraction would turn a small release into a huge available budget.
    pub const fn sub(self, other: Self) -> Result<Self, ArithmeticError> {
        match self.scaled.checked_sub(other.scaled) {
            Some(v) => Ok(Self { scaled: v }),
            None => Err(ArithmeticError::Overflow),
        }
    }

    #[must_use]
    pub const fn scaled(self) -> i128 {
        self.scaled
    }
}

/// Carried in the type so a unit cannot be mismatched. A production control once never fired:
/// thresholds were percentages, the compared value a fraction, and nothing errored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Money,
    Ratio,
}

/// Cost of `quantity` at `price`, at [`Amount::SCALE`]. Narrowing rounds away from zero so a spend
/// is never reserved for less than it costs. Widening refuses on overflow.
pub fn notional(
    quantity_scaled: i128,
    quantity_scale: u8,
    price_scaled: i128,
    price_scale: u8,
) -> Result<Amount, ArithmeticError> {
    let product = quantity_scaled.checked_mul(price_scaled).ok_or(ArithmeticError::Overflow)?;

    let from = i32::from(quantity_scale) + i32::from(price_scale);
    let to = i32::from(Amount::SCALE);

    let scaled = if to >= from {
        let factor = pow10(u32::try_from(to - from).map_err(|_| ArithmeticError::Overflow)?)?;
        product.checked_mul(factor).ok_or(ArithmeticError::Overflow)?
    } else {
        let factor = pow10(u32::try_from(from - to).map_err(|_| ArithmeticError::Overflow)?)?;
        let quotient = product / factor;
        let remainder = product % factor;
        if remainder == 0 {
            quotient
        } else if product > 0 {
            quotient.checked_add(1).ok_or(ArithmeticError::Overflow)?
        } else {
            quotient.checked_sub(1).ok_or(ArithmeticError::Overflow)?
        }
    };

    Ok(Amount::from_scaled(scaled))
}

pub(crate) const fn pow10(exponent: u32) -> Result<i128, ArithmeticError> {
    let mut value: i128 = 1;
    let mut remaining = exponent;
    while remaining > 0 {
        match value.checked_mul(10) {
            Some(next) => value = next,
            None => return Err(ArithmeticError::Overflow),
        }
        remaining -= 1;
    }
    Ok(value)
}

/// A distinct type from [`Amount`], not an alias, so the compiler catches unit mixing. 500 is five
/// per cent. One stated scale settles whether a percentage means 5 or 0.05.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ratio {
    pub(crate) scaled: i128,
}

impl Ratio {
    pub const ONE: i128 = 10_000;

    #[must_use]
    pub const fn from_scaled(scaled: i128) -> Self {
        Self { scaled }
    }

    #[must_use]
    pub const fn scaled(self) -> i128 {
        self.scaled
    }
}

/// Every threshold and observation crossing this crate is a `Threshold`, so no bare number meets another kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    Money(Amount),
    Ratio(Ratio),
}

impl Threshold {
    #[must_use]
    pub const fn unit(self) -> Unit {
        match self {
            Self::Money(_) => Unit::Money,
            Self::Ratio(_) => Unit::Ratio,
        }
    }

    /// Private: comparing two without agreeing their units is the mistake this module prevents.
    pub(crate) const fn scaled(self) -> i128 {
        match self {
            Self::Money(amount) => amount.scaled,
            Self::Ratio(ratio) => ratio.scaled,
        }
    }
}
