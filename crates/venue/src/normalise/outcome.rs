// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Normalised venue outcomes. The adapter contract ends venue error taxonomies at the adapter.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueOutcome {
    Accepted,
    Rejected(Rejection),
    /// Produced by the platform, not a venue. A paper run reporting any other outcome would state
    /// a fact it lacks and read as live.
    NotSent,
    /// Never a rejection: the order may have reached the venue and filled.
    /// Treating it as a refusal invents a fact about an operator's capital.
    Unknown(Unknown),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    RateLimited {
        /// Where the venue says when to retry, in nanoseconds. `None` where it does not.
        retry_after_ns: Option<i128>,
    },
    InsufficientMargin,
    ReduceOnlyViolated,
    UnknownInstrument,
    PriceOutOfBand,
    /// Distinct from `PriceOutOfBand`: one is repriced and the other resized, and the wrong
    /// remedy retries into the same refusal.
    BelowMinimumSize,
    /// The venue refused the client identifier - venue assurance's rejection constraint, arriving late.
    IdentifierRejected,
    /// Authentication precedes matching, so the order did not happen. As `Unknown`, every
    /// mistyped key would reach reconciliation. A retry cannot succeed.
    CredentialRefused,
    /// The venue already has this id. After an `Unknown` it confirms the first attempt landed.
    Duplicate,
    /// The code travels with it and is never mapped to the nearest class: guessing is how a
    /// platform answers confidently and wrongly.
    Unclassified {
        venue_code: String,
    },
}

/// The attempted action decides what a refusal means: a refused submit settles the order, a
/// refused cancel does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempted {
    Submit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    Timeout,
    ConnectionLost,
    Unreadable,
}

impl VenueOutcome {
    /// Adds the venue's own statement of when to retry to a rate-limit rejection that carries none.
    /// Any other outcome is returned unchanged.
    #[must_use]
    pub fn with_retry_after(self, retry_after_ns: Option<i128>) -> Self {
        match self {
            VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None }) => {
                VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns })
            }
            other => other,
        }
    }

    /// Only a recognised rejection permits it. An `Unknown` never does.
    pub fn is_known_not_to_have_happened(&self) -> bool {
        matches!(self, VenueOutcome::Rejected(_))
    }

    /// Reconciliation's Extra class: capital may have moved unseen. The attempted action is a parameter
    /// because an unreadable refusal settles a submit but not a cancel, which may have filled.
    pub fn needs_reconciliation(&self, attempted: Attempted) -> bool {
        match self {
            VenueOutcome::Unknown(_) => true,
            VenueOutcome::Rejected(Rejection::Unclassified { .. }) => {
                matches!(attempted, Attempted::Cancel)
            }
            // A never-sent order's fate is closed. Queueing it would chase a venue that never
            // heard of it.
            VenueOutcome::NotSent => false,
            _ => false,
        }
    }

    /// Whether no order exists at the venue for this identifier. Narrower than
    /// `is_known_not_to_have_happened`: a duplicate created no second order, but the first one
    /// exists, so what it reserved is still at risk.
    pub fn leaves_no_order(&self) -> bool {
        matches!(self, VenueOutcome::Rejected(r) if !matches!(r, Rejection::Duplicate))
    }

    /// Safe after an unknown, since the venue answers `Duplicate` if the first attempt landed.
    pub fn may_resubmit_idempotently(&self) -> bool {
        matches!(self, VenueOutcome::Unknown(_))
    }
}

/// The answer for an order whose terms the venue does not take: refused before it is sent, naming
/// the term, never sent under a near one (decision 0009).
#[must_use]
pub fn terms_not_taken(terms: bijective_model::OrderTerms) -> VenueOutcome {
    let post = if terms.post_only { ", post-only" } else { "" };
    VenueOutcome::Rejected(Rejection::Unclassified {
        venue_code: format!(
            "not sent: this venue does not take time_in_force {}{post}",
            terms.time_in_force.name()
        ),
    })
}
