// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Refusing a configured value, with the reason.
//! Configuration assurance: a rule returns a reason, never a boolean or `Option`, so a refusal says what is wrong and why.

use std::fmt;

/// Why a value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// Unset, where a safety property depends on it.
    /// Configuration assurance: absent is unsafe, not default. Where no safe default exists the platform does not start.
    Absent,
    NotPositive {
        found: i128,
    },
    OutOfRange {
        found: i128,
        minimum: i128,
        maximum: i128,
    },
    /// A list a safety property depends on, with nothing in it.
    Empty,
    Duplicate {
        value: String,
    },
    /// A name that is not among those the platform knows. Carries what it knows so the operator need not look.
    Unknown {
        value: String,
        known: Vec<String>,
    },
    /// The text could not be read at all. Carries what the parser said.
    Malformed {
        detail: String,
    },
    /// Two settings that cannot both be given. Names the other so the operator can act.
    Conflicting {
        with: String,
    },
    /// A setting that means nothing without another, which is not there.
    Requires {
        other: String,
    },
}

/// A refused value, and where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub field: String,
    pub reason: Reason,
}

impl Invalid {
    pub fn new(field: impl Into<String>, reason: Reason) -> Self {
        Self { field: field.into(), reason }
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.field)?;
        match &self.reason {
            Reason::Absent => write!(f, "is not set, and nothing safe can be assumed for it"),
            Reason::NotPositive { found } => write!(f, "must be greater than zero, and is {found}"),
            Reason::OutOfRange { found, minimum, maximum } => {
                write!(f, "is {found}, outside {minimum}..={maximum}")
            }
            Reason::Empty => write!(f, "is empty, and a safety property depends on it"),
            Reason::Duplicate { value } => write!(f, "names {value} twice"),
            Reason::Unknown { value, known } => {
                write!(f, "names {value}, which is not one of: {}", known.join(", "))
            }
            Reason::Malformed { detail } => write!(f, "could not be read: {detail}"),
            Reason::Conflicting { with } => {
                write!(f, "is given with {with}, and only one of the two may be")
            }
            Reason::Requires { other } => write!(f, "is given without {other}, which it needs"),
        }
    }
}

impl std::error::Error for Invalid {}

/// A value a safety property depends on must be set.
pub fn required<T>(field: &str, value: Option<T>) -> Result<T, Invalid> {
    value.ok_or_else(|| Invalid::new(field, Reason::Absent))
}

pub fn positive(field: &str, value: i128) -> Result<i128, Invalid> {
    if value > 0 {
        Ok(value)
    } else {
        Err(Invalid::new(field, Reason::NotPositive { found: value }))
    }
}

pub fn within(field: &str, value: i128, minimum: i128, maximum: i128) -> Result<i128, Invalid> {
    if value >= minimum && value <= maximum {
        Ok(value)
    } else {
        Err(Invalid::new(field, Reason::OutOfRange { found: value, minimum, maximum }))
    }
}

pub fn non_empty<'a, T>(field: &str, values: &'a [T]) -> Result<&'a [T], Invalid> {
    if values.is_empty() { Err(Invalid::new(field, Reason::Empty)) } else { Ok(values) }
}

/// Every name in `values` appears once.
pub fn distinct(field: &str, values: &[String]) -> Result<(), Invalid> {
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        if !seen.insert(value.as_str()) {
            return Err(Invalid::new(field, Reason::Duplicate { value: value.clone() }));
        }
    }
    Ok(())
}

/// `value` is one the platform knows about.
/// Takes anything a name can be read out of, so callers holding `&[&str]` can use the shared rule.
pub fn known<S: AsRef<str>>(field: &str, value: &str, known_values: &[S]) -> Result<(), Invalid> {
    if known_values.iter().any(|k| k.as_ref() == value) {
        Ok(())
    } else {
        let mut listed: Vec<String> = known_values.iter().map(|k| k.as_ref().to_string()).collect();
        listed.sort();
        Err(Invalid::new(field, Reason::Unknown { value: value.to_string(), known: listed }))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/configuration/tests.rs"]
mod tests;
