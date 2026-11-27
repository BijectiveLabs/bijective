// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reads venue answers into outcomes. HTTP 200 with status ok does not mean the action worked.
//! Kept apart from the client so captured answers can be checked offline.

use bijective_venue::normalise::{Rejection, Unknown, VenueOutcome};
use serde_json::Value;

/// Every unsettled shape maps to one Unknown: the contract carries no detail and the decision
/// is the same in all four.
const DO_NOT_KNOW: VenueOutcome = VenueOutcome::Unknown(Unknown::Unreadable);

/// The attempted action is not a parameter: a refused cancel may mean the order filled,
/// which `needs_reconciliation` asks at the caller.
pub fn read_answer(http_status: u16, body: &str) -> VenueOutcome {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        // Unreadable is not a refusal: the request may have executed.
        let _ = http_status;
        return DO_NOT_KNOW;
    };

    if parsed.get("status").and_then(Value::as_str) != Some("ok") {
        let detail = parsed.get("response").and_then(Value::as_str).unwrap_or(body);
        return refusal(detail);
    }

    let statuses = parsed.pointer("/response/data/statuses").and_then(Value::as_array);

    match statuses {
        // Envelope ok but nothing about our item: unknown, never a refusal.
        None => DO_NOT_KNOW,
        Some(entries) if entries.len() == 1 => read_entry(&entries[0]),
        // Position is the only attribution. With other than one entry, do not guess.
        Some(_) => DO_NOT_KNOW,
    }
}

fn read_entry(entry: &Value) -> VenueOutcome {
    if let Some(text) = entry.as_str() {
        // Two bare strings mean live with no id yet.
        return match text {
            "waitingForFill" | "waitingForTrigger" | "success" => VenueOutcome::Accepted,
            other => refusal(other),
        };
    }
    if let Some(error) = entry.get("error").and_then(Value::as_str) {
        return refusal(error);
    }
    if entry.get("resting").is_some() || entry.get("filled").is_some() {
        return VenueOutcome::Accepted;
    }
    DO_NOT_KNOW
}

/// Keyed on the venue's status names, stable where its sentences are not.
/// An unknown code travels whole so a changed venue is visible.
fn refusal(detail: &str) -> VenueOutcome {
    let lowered = detail.to_ascii_lowercase();

    // Auth is checked before matching, so the order did not happen. A bad signature looks like
    // a missing account.
    if lowered.contains("does not exist") || lowered.contains("must deposit") {
        return VenueOutcome::Rejected(Rejection::CredentialRefused);
    }
    if lowered.contains("rate limit") || lowered.contains("too many requests") {
        return VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None });
    }
    if detail.contains("PerpMargin") || lowered.contains("insufficient margin") {
        return VenueOutcome::Rejected(Rejection::InsufficientMargin);
    }
    if detail.contains("Oracle") || lowered.contains("too far from oracle") {
        return VenueOutcome::Rejected(Rejection::PriceOutOfBand);
    }
    if detail.contains("MinTradeNtl") || lowered.contains("minimum value") {
        return VenueOutcome::Rejected(Rejection::BelowMinimumSize);
    }
    if detail.contains("ReduceOnly") || lowered.contains("reduce only") {
        return VenueOutcome::Rejected(Rejection::ReduceOnlyViolated);
    }
    if lowered.contains("unknown asset") || lowered.contains("invalid coin") {
        return VenueOutcome::Rejected(Rejection::UnknownInstrument);
    }
    if lowered.contains("already") && lowered.contains("cloid") {
        return VenueOutcome::Rejected(Rejection::Duplicate);
    }
    VenueOutcome::Rejected(Rejection::Unclassified { venue_code: detail.to_string() })
}

// Flat file: a submodule would otherwise be looked up in a directory this crate lacks.
#[cfg(test)]
#[path = "../tests/unit/exchange_tests.rs"]
mod exchange_tests;
