// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The suite the adapter contract requires: written once against the contract, run against every adapter.

use crate::normalise::VenueOutcome;
use crate::profile::DataClass;

use crate::contract::{
    ConnectionState, Continuity, MarketFeed, MarketRecord, OrderRequest, VenueData, VenueExecution,
    VenueSymbol, check_record, class_of,
};
use crate::profile::CapabilityDeclaration;

/// One check and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,
    pub outcome: CheckOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Passed,
    Failed {
        detail: String,
    },
    /// Reported, not skipped: a check that could not run must not read as passed.
    RequiresLiveVenue,
}

impl CheckResult {
    fn pass(name: &'static str) -> Self {
        Self { name, outcome: CheckOutcome::Passed }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self { name, outcome: CheckOutcome::Failed { detail: detail.into() } }
    }

    fn live(name: &'static str) -> Self {
        Self { name, outcome: CheckOutcome::RequiresLiveVenue }
    }

    pub fn is_pass(&self) -> bool {
        matches!(self.outcome, CheckOutcome::Passed)
    }
}

/// Supplied by the caller from the venue assurance verifier, so this crate does not depend on an operational component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationCheck {
    /// Not attempted or venue unobserved. Both land here since no observation is never a healthy state.
    Unverified,
    Matches,
    Diverges {
        detail: String,
    },
}

/// Run the data-side checks. `Unverified` is the accurate default.
pub async fn check_data(
    adapter: &mut dyn VenueData,
    instrument: &VenueSymbol,
    now_ns: i128,
    declaration: &DeclarationCheck,
) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // The confirmed-subscription rule: an unconfirmed subscription is not subscribed, so no usable state on the strength of asking.
    match adapter.connect(now_ns).await {
        Ok(_) => {}
        Err(e) => return vec![CheckResult::fail("connect", e.to_string())],
    }

    let served = adapter.declaration().serves(DataClass::TopOfBook);
    match adapter.subscribe(DataClass::TopOfBook, instrument, now_ns).await {
        Ok(ack) => {
            if ack.is_attached() {
                out.push(CheckResult::pass("subscribe-acknowledged"));
            } else {
                out.push(CheckResult::fail(
                    "subscribe-acknowledged",
                    "the adapter returned an unconfirmed subscription and did not say so",
                ));
            }
        }
        Err(e) if !served => {
            out.push(CheckResult::pass("subscribe-refused-when-not-served"));
            let _ = e;
        }
        Err(e) => out.push(CheckResult::fail("subscribe-acknowledged", e.to_string())),
    }

    // A reconnect must state whether it crossed a gap. Always answering Unbroken asserts the unknowable.
    match adapter.reconnect(now_ns + 1).await {
        Ok(r) => {
            let consistent = match r.continuity {
                Continuity::Unbroken => matches!(r.state, ConnectionState::Connected { .. }),
                _ => true,
            };
            if consistent {
                out.push(CheckResult::pass("reconnect-declares-continuity"));
            } else {
                out.push(CheckResult::fail(
                    "reconnect-declares-continuity",
                    "claimed an unbroken stream while not connected",
                ));
            }
        }
        Err(e) => out.push(CheckResult::fail("reconnect-declares-continuity", e.to_string())),
    }

    if let ConnectionState::Degraded { .. } = adapter.state() {
        if adapter.state().is_usable() {
            out.push(CheckResult::fail("degraded-is-not-usable", "degraded reported as usable"));
        } else {
            out.push(CheckResult::pass("degraded-is-not-usable"));
        }
    } else {
        out.push(CheckResult::pass("degraded-is-not-usable"));
    }

    out.push(match declaration {
        DeclarationCheck::Matches => CheckResult::pass("declaration-matches-live-behaviour"),
        DeclarationCheck::Diverges { detail } => {
            CheckResult::fail("declaration-matches-live-behaviour", detail.clone())
        }
        DeclarationCheck::Unverified => CheckResult::live("declaration-matches-live-behaviour"),
    });
    out
}

/// Run the execution-side checks against any adapter.
pub async fn check_execution(
    adapter: &mut dyn VenueExecution,
    clearance: &crate::contract::Clearance,
    order: &OrderRequest,
    now_ns: i128,
) -> Vec<CheckResult> {
    let mut out = Vec::new();

    let first = adapter.submit(clearance, order, now_ns).await;

    // A resubmitted client id must not create a second order. Duplicate is a confirmation.
    let second = adapter.submit(clearance, order, now_ns + 1).await;
    let idempotent = match (&first, &second) {
        (VenueOutcome::Accepted, VenueOutcome::Rejected(r)) => {
            matches!(r, crate::normalise::Rejection::Duplicate)
        }
        (VenueOutcome::Unknown(_), _) => true,
        _ => false,
    };
    if idempotent {
        out.push(CheckResult::pass("resubmission-is-idempotent"));
    } else {
        out.push(CheckResult::fail(
            "resubmission-is-idempotent",
            "the same client identifier was accepted twice",
        ));
    }

    // An unknown must never be reported as a rejection, because the order may have landed.
    if let VenueOutcome::Unknown(_) = first
        && first.is_known_not_to_have_happened()
    {
        out.push(CheckResult::fail(
            "unknown-is-not-a-rejection",
            "an unknown outcome claimed the order did not happen",
        ));
    } else {
        out.push(CheckResult::pass("unknown-is-not-a-rejection"));
    }

    match adapter.account(now_ns + 2).await {
        Ok(_) => out.push(CheckResult::pass("account-readable-for-reconciliation")),
        Err(e) => out.push(CheckResult::fail("account-readable-for-reconciliation", e.to_string())),
    }

    out.push(CheckResult::live("disconnect-mid-order"));
    out
}

/// One frame as recorded from the venue, with the arrival time the recorder stamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedFrame<'a> {
    pub received_at_ns: i128,
    pub frame: &'a [u8],
}

/// Replays recorded traffic through a feed and holds every record it emits to the record
/// contract. Pure over the frames, so it runs in a test and against a fresh capture alike.
pub fn check_feed(
    feed: &mut dyn MarketFeed,
    frames: &[RecordedFrame<'_>],
    declaration: &CapabilityDeclaration,
) -> Vec<CheckResult> {
    let mut emitted: Vec<(i128, MarketRecord)> = Vec::new();
    let mut refusal: Option<String> = None;
    for (index, recorded) in frames.iter().enumerate() {
        match feed.offer(recorded.frame, recorded.received_at_ns) {
            Ok(offered) => {
                emitted.extend(offered.records.into_iter().map(|r| (recorded.received_at_ns, r)));
            }
            Err(e) => {
                refusal.get_or_insert_with(|| format!("frame {index}: {e}"));
            }
        }
    }

    vec![
        match refusal {
            None => CheckResult::pass("recording-accepted"),
            Some(detail) => CheckResult::fail("recording-accepted", detail),
        },
        // No vacuous pass: a feed that emits nothing satisfies every rule below.
        if emitted.is_empty() {
            CheckResult::fail("records-observed", "the recording produced no market record")
        } else {
            CheckResult::pass("records-observed")
        },
        first_failure("records-well-formed", &emitted, |_, r| {
            check_record(r).err().map(|d| d.to_string())
        }),
        first_failure("records-stamped-on-arrival", &emitted, |at, r| {
            (r.received_at_ns != at)
                .then(|| format!("stamped {} for a frame received at {at}", r.received_at_ns))
        }),
        first_failure("records-declared", &emitted, |_, r| {
            let class = class_of(&r.fact);
            (!declaration.serves(class)).then(|| format!("{class:?} is emitted and not declared"))
        }),
    ]
}

fn first_failure(
    name: &'static str,
    emitted: &[(i128, MarketRecord)],
    defect: impl Fn(i128, &MarketRecord) -> Option<String>,
) -> CheckResult {
    let found = emitted.iter().enumerate().find_map(|(index, (at, record))| {
        defect(*at, record).map(|d| format!("record {index} on {}: {d}", record.instrument))
    });
    match found {
        None => CheckResult::pass(name),
        Some(detail) => CheckResult::fail(name, detail),
    }
}

/// A check needing a live venue does not count as a pass.
pub fn is_releasable(results: &[CheckResult]) -> bool {
    !results.is_empty() && results.iter().all(CheckResult::is_pass)
}
