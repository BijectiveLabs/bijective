// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The open format of the `detections` stream: what a detector, any program's, concluded about
//! one subject, one JSON object per entry. See `docs/concepts/observability.md`, "Detections".
//!
//! The console reads the stream and writes nothing to it. `encode` is here so that a writer,
//! the platform's or a third party's, produces exactly what `decode` accepts.

use serde_json::{Map, Value, json};

/// The stream the console reads detections from.
pub const DETECTIONS: &str = "detections";

/// The detections about the node of `mode`: `detections` for live, `detections-<mode>` for the
/// others, kept apart like the node's own streams.
#[must_use]
pub fn stream_of(mode: &str) -> String {
    if mode == "live" { DETECTIONS.to_string() } else { format!("{DETECTIONS}-{mode}") }
}

/// The format's version, in every entry. A change that breaks a reader raises it.
pub const DETECTION_FORMAT: u64 = 1;

/// What a detection asks of an operator, or that an earlier one no longer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raised {
    Attention,
    Alarm,
    Cleared,
}

impl Raised {
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Alarm => "alarm",
            Self::Cleared => "cleared",
        }
    }

    fn of(word: &str) -> Option<Self> {
        [Self::Attention, Self::Alarm, Self::Cleared].into_iter().find(|r| r.word() == word)
    }
}

/// One conclusion. Figures are exact decimals as written, never floats. Positions name the
/// entries of `source` the conclusion was drawn from, so it can be checked against the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub detector: String,
    /// What it is about, as `<kind>:<name>`, such as `venue:<name>` or `order:<client id>`.
    pub subject: String,
    pub level: Raised,
    /// One sentence an operator can act on.
    pub detail: String,
    pub measured: Option<String>,
    pub threshold: Option<String>,
    /// The stream the evidence is in.
    pub source: String,
    pub evidence: Vec<u64>,
    /// When the evidence says it happened, UTC nanoseconds: the record's time, not the detector's.
    pub at_ns: i128,
}

/// Why an entry is not a detection. The console lists it as unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused(pub &'static str);

/// The entry as one line of JSON. Integers as decimal strings, like the console's tables.
#[must_use]
pub fn encode(detection: &Detection) -> Vec<u8> {
    let evidence: Vec<String> = detection.evidence.iter().map(u64::to_string).collect();
    let value = json!({
        "format": DETECTION_FORMAT,
        "detector": detection.detector,
        "subject": detection.subject,
        "level": detection.level.word(),
        "detail": detection.detail,
        "measured": detection.measured,
        "threshold": detection.threshold,
        "source": detection.source,
        "evidence": evidence,
        "at_ns": detection.at_ns.to_string(),
    });
    value.to_string().into_bytes()
}

/// Accepts exactly what `encode` writes: every field present, names and figures well formed.
///
/// # Errors
/// `Refused` naming the first field that is missing or malformed.
pub fn decode(payload: &[u8]) -> Result<Detection, Refused> {
    let value: Value = serde_json::from_slice(payload).map_err(|_| Refused("not JSON"))?;
    let object = value.as_object().ok_or(Refused("not an object"))?;
    if object.get("format").and_then(Value::as_u64) != Some(DETECTION_FORMAT) {
        return Err(Refused("format"));
    }
    let level = Raised::of(&named(object, "level")?).ok_or(Refused("level"))?;
    let at_ns = text(object, "at_ns")?.parse::<i128>().map_err(|_| Refused("at_ns"))?;
    let evidence = object
        .get("evidence")
        .and_then(Value::as_array)
        .ok_or(Refused("evidence"))?
        .iter()
        .map(|p| p.as_str().and_then(|s| s.parse::<u64>().ok()).ok_or(Refused("evidence")))
        .collect::<Result<Vec<u64>, Refused>>()?;
    Ok(Detection {
        detector: named(object, "detector")?,
        subject: named(object, "subject")?,
        level,
        detail: text(object, "detail")?,
        measured: figure(object, "measured")?,
        threshold: figure(object, "threshold")?,
        source: named(object, "source")?,
        evidence,
        at_ns,
    })
}

fn text(object: &Map<String, Value>, field: &'static str) -> Result<String, Refused> {
    object.get(field).and_then(Value::as_str).map(str::to_string).ok_or(Refused(field))
}

/// A text field that may not be empty.
fn named(object: &Map<String, Value>, field: &'static str) -> Result<String, Refused> {
    text(object, field).and_then(|t| if t.is_empty() { Err(Refused(field)) } else { Ok(t) })
}

/// An exact decimal written out, or `null` where the detector measured none.
fn figure(object: &Map<String, Value>, field: &'static str) -> Result<Option<String>, Refused> {
    match object.get(field) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) if is_decimal(raw) => Ok(Some(raw.clone())),
        _ => Err(Refused(field)),
    }
}

pub(crate) fn is_decimal(raw: &str) -> bool {
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    let mut parts = digits.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next();
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    parts.next().is_none() && all_digits(whole) && fraction.is_none_or(all_digits)
}

#[cfg(test)]
#[path = "../tests/unit/detection_tests.rs"]
mod tests;
