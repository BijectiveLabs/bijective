// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Exact decimal parsing. There is no `f64` on any path here, and that is the point.

/// Digits after the point, read from the venue profile and never hardcoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scale(u8);

/// The widest scale `i128` can carry with room for a realistic magnitude.
const MAX_SCALE: u8 = 18;

/// A reason rather than a bare `None`, so every caller reports the same fault the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleError {
    TooWide { asked: u8, maximum: u8 },
}

impl core::fmt::Display for ScaleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooWide { asked, maximum } => write!(
                f,
                "a scale of {asked} digits cannot be represented; this build carries at most {maximum}"
            ),
        }
    }
}

impl std::error::Error for ScaleError {}

impl Scale {
    /// Refused, never narrowed: rounding an unrepresentable precision hides a configuration
    /// error.
    pub fn new(digits: u8) -> Result<Self, ScaleError> {
        if digits > MAX_SCALE {
            return Err(ScaleError::TooWide { asked: digits, maximum: MAX_SCALE });
        }
        Ok(Self(digits))
    }

    pub fn digits(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalError {
    Empty,
    NotANumber {
        at: usize,
        found: char,
    },
    /// Refused, not rounded: a dropped digit makes a benchmark measure a fiction. A venue exceeding
    /// its declared precision is a profile defect.
    ExcessPrecision {
        declared: u8,
        needed: u8,
    },
    Overflow,
    Malformed,
}

impl core::fmt::Display for DecimalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecimalError::Empty => write!(f, "value is empty"),
            DecimalError::NotANumber { at, found } => {
                write!(f, "character {found:?} at position {at} is not part of a decimal")
            }
            DecimalError::ExcessPrecision { declared, needed } => {
                write!(f, "venue sent {needed} decimal places, the profile declares {declared}")
            }
            DecimalError::Overflow => write!(f, "value does not fit the platform scale"),
            DecimalError::Malformed => write!(f, "value is not a well-formed decimal"),
        }
    }
}

impl std::error::Error for DecimalError {}

/// Exponent notation moves the point in the digit string, never through a float.
pub fn parse_scaled(text: &str, scale: Scale) -> Result<i128, DecimalError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(DecimalError::Empty);
    }

    let (mantissa, exponent) = split_exponent(text)?;
    let (negative, digits, point_at) = split_mantissa(mantissa)?;

    let frac_len = digits.len() as i64 - point_at as i64;
    let effective_frac = frac_len - exponent;

    if effective_frac > scale.digits() as i64 {
        // Trailing zeros are not precision: "1.2300" at scale 2 is exact.
        let significant = trim_trailing_zeros(&digits, effective_frac, scale.digits() as i64);
        if significant > scale.digits() as i64 {
            return Err(DecimalError::ExcessPrecision {
                declared: scale.digits(),
                needed: significant.min(u8::MAX as i64) as u8,
            });
        }
    }

    let mut value: i128 = 0;
    for &d in &digits {
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(d as i128))
            .ok_or(DecimalError::Overflow)?;
    }

    let shift = scale.digits() as i64 - effective_frac;
    if shift > 0 {
        for _ in 0..shift {
            value = value.checked_mul(10).ok_or(DecimalError::Overflow)?;
        }
    } else {
        for _ in 0..(-shift) {
            // Only reached where the digits removed are zeros, checked above.
            value /= 10;
        }
    }

    Ok(if negative { -value } else { value })
}

/// Render a scaled integer back to a decimal string, in the form a venue expects.
pub fn render_scaled(value: i128, scale: Scale) -> String {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let digits = scale.digits() as u32;

    if digits == 0 {
        return format!("{}{magnitude}", if negative { "-" } else { "" });
    }

    let divisor = 10u128.pow(digits);
    let whole = magnitude / divisor;
    let frac = magnitude % divisor;
    format!("{}{whole}.{frac:0width$}", if negative { "-" } else { "" }, width = digits as usize)
}

fn split_exponent(text: &str) -> Result<(&str, i64), DecimalError> {
    let Some(at) = text.find(['e', 'E']) else {
        return Ok((text, 0));
    };
    let (mantissa, rest) = text.split_at(at);
    let exp_text = &rest[1..];
    if exp_text.is_empty() || mantissa.is_empty() {
        return Err(DecimalError::Malformed);
    }
    let exponent: i64 = exp_text.parse().map_err(|_| DecimalError::Malformed)?;
    if exponent.abs() > 40 {
        return Err(DecimalError::Overflow);
    }
    Ok((mantissa, exponent))
}

fn split_mantissa(text: &str) -> Result<(bool, Vec<u8>, usize), DecimalError> {
    let mut negative = false;
    let mut body = text;
    if let Some(rest) = body.strip_prefix('-') {
        negative = true;
        body = rest;
    } else if let Some(rest) = body.strip_prefix('+') {
        body = rest;
    }
    if body.is_empty() {
        return Err(DecimalError::Empty);
    }

    let mut digits = Vec::with_capacity(body.len());
    let mut point_at = None;
    for (at, ch) in body.chars().enumerate() {
        match ch {
            '0'..='9' => digits.push(ch as u8 - b'0'),
            '.' if point_at.is_none() => point_at = Some(digits.len()),
            '.' => return Err(DecimalError::Malformed),
            other => return Err(DecimalError::NotANumber { at, found: other }),
        }
    }
    if digits.is_empty() {
        return Err(DecimalError::Malformed);
    }
    let point_at = point_at.unwrap_or(digits.len());
    Ok((negative, digits, point_at))
}

fn trim_trailing_zeros(digits: &[u8], frac_len: i64, floor: i64) -> i64 {
    let mut len = frac_len;
    let mut idx = digits.len();
    while len > floor && idx > 0 && digits[idx - 1] == 0 {
        len -= 1;
        idx -= 1;
    }
    len
}
