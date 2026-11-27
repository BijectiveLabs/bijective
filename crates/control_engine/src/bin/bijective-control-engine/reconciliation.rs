// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reconciliation at start: each live venue's evidence read against the orders routed to it, and
//! the outcome recorded before the first decision pass.
//!
//! A paper run and a backtest reconcile against their simulated venues, which hold exactly the
//! orders they placed.

use std::collections::{BTreeMap, BTreeSet};

use bijective_control_engine::OrderPath;
use bijective_control_engine::wire::EvidenceRead;
use bijective_lifecycle::Verdict;
use bijective_venue::contract::{ConnectError, Pushed, VenueEvidence, VenueExecution, contain};
use bijective_venue::resilience::{Backoff, SplitMix64};

use crate::clock::now_ns;
use crate::venues::OpenVenues;
use crate::{Fatal, PROGRAM};

/// Every open venue: at start, so a restart begins from what the venues hold, and on the
/// position-check cadence, so a change made outside the platform is found while it trades.
pub(crate) async fn reconcile_every(
    path: &mut OrderPath,
    adapters: &mut OpenVenues,
    faulted: &mut BTreeSet<String>,
    label: &str,
) -> Result<String, Fatal> {
    let every: Vec<String> = adapters.iter().map(|(name, _)| name.clone()).collect();
    reconcile_venues(path, adapters, faulted, &every, label).await
}

/// The venues where a pass could still settle something, with those whose push channel reported a
/// change. `Ok(None)` when there are none.
pub(crate) async fn reconcile_unsettled(
    path: &mut OrderPath,
    adapters: &mut OpenVenues,
    faulted: &mut BTreeSet<String>,
    reported: &[String],
) -> Result<Option<String>, Fatal> {
    let mut wanted = path.unsettled_venues();
    wanted.extend(reported.iter().cloned());
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(None);
    }
    reconcile_venues(path, adapters, faulted, &wanted, "reconciled again").await.map(Some)
}

/// Only the venues whose push channel reported a change, read on the next pass instead of the next
/// interval. `Ok(None)` when none did.
pub(crate) async fn reconcile_reported(
    path: &mut OrderPath,
    adapters: &mut OpenVenues,
    faulted: &mut BTreeSet<String>,
    reported: &[String],
) -> Result<Option<String>, Fatal> {
    if reported.is_empty() {
        return Ok(None);
    }
    reconcile_venues(path, adapters, faulted, reported, "reconciled on a venue report")
        .await
        .map(Some)
}

/// Asks every open venue's push channel what it said, and returns those that reported a change.
/// A channel that lapses or returns is logged once per transition. `channels` holds the last state.
pub(crate) fn pushed_changes(
    adapters: &mut OpenVenues,
    faulted: &BTreeSet<String>,
    channels: &mut BTreeMap<String, Pushed>,
) -> Vec<String> {
    let mut reported = Vec::new();
    for (venue, adapter) in adapters.iter_mut() {
        if faulted.contains(venue) {
            continue;
        }
        let pushed = adapter.pushed();
        if pushed == Pushed::Changed {
            reported.push(venue.clone());
        }
        let attached = matches!(pushed, Pushed::Quiet | Pushed::Changed);
        let before = channels.insert(venue.clone(), pushed);
        let was_attached = matches!(before, Some(Pushed::Quiet | Pushed::Changed));
        if pushed != Pushed::NotKept && before.is_some() && attached != was_attached {
            let now = if attached { "attached" } else { "lapsed; read on the interval" };
            eprintln!("{PROGRAM}: {venue} private stream {now}");
        }
    }
    reported
}

/// One clause per venue for the readiness detail. A wanted venue that is not open is reported,
/// never skipped: what it holds is still unsettled.
async fn reconcile_venues(
    path: &mut OrderPath,
    adapters: &mut OpenVenues,
    faulted: &mut BTreeSet<String>,
    wanted: &[String],
    label: &str,
) -> Result<String, Fatal> {
    let mut clauses = Vec::new();
    for venue in wanted {
        let Some((_, adapter)) = adapters.iter_mut().find(|(name, _)| name == venue) else {
            clauses.push(format!("{venue} not open"));
            continue;
        };
        if faulted.contains(venue) {
            clauses.push(format!("{venue} faulted"));
            continue;
        }
        let since = path.evidence_since_ns(venue).unwrap_or_else(now_ns);
        let (read, now) = read_evidence(adapter.as_mut(), since).await;
        let clause = match read {
            Ok(Ok(evidence)) => {
                let done = path.reconcile_venue(venue, &evidence, now).map_err(unrecorded)?;
                for finding in &done.findings {
                    eprintln!("{PROGRAM}: reconciliation {venue}: {finding}");
                }
                format!(
                    "{venue} {}, {} change(s) recorded",
                    verdict_name(done.verdict, done.findings.len()),
                    done.recorded
                )
            }
            // An adapter that publishes no evidence has said so. Any other refusal is a read
            // that failed, which says nothing about what the venue holds.
            Ok(Err(ConnectError::NotPermitted { detail })) => {
                unavailable(path, venue, &detail, EvidenceRead::NotPublished, now)?
            }
            Ok(Err(e)) => unavailable(path, venue, &e.to_string(), EvidenceRead::Failed, now)?,
            Err(_) => {
                // The adapter panicked: its state is unknown, so it is never called again.
                faulted.insert(venue.clone());
                let reason = "the adapter failed while reading evidence";
                unavailable(path, venue, reason, EvidenceRead::Failed, now)?
            }
        };
        clauses.push(clause);
    }
    if clauses.is_empty() {
        return Ok("not reconciled: no venue is open".into());
    }
    Ok(format!("{label}: {}", clauses.join(", ")))
}

/// Attempts after the first when a read fails in transport (seen 2026-11-19: one of four accounts
/// opened at once answered nothing on its first read, and the node started halted). A read is
/// idempotent, so it is retried. A venue that refuses, or an adapter that panicked, is not.
const READ_RETRIES: u32 = 2;

/// Half a second, then a second, each spread over its upper half.
const READ_BACKOFF: Result<Backoff, bijective_venue::resilience::BackoffError> =
    Backoff::new(500_000_000, 1_000_000_000);

/// One evidence read, retried on transport failure within `READ_RETRIES`, with the instant the
/// answer that is kept was asked for.
async fn read_evidence(
    adapter: &mut dyn VenueExecution,
    since_ns: i128,
) -> (Result<Result<VenueEvidence, ConnectError>, bijective_venue::contract::Panicked>, i128) {
    let mut jitter = SplitMix64::for_this_process(now_ns());
    let mut failures = 0;
    loop {
        let now = now_ns();
        let read = contain(adapter.evidence(since_ns, now)).await;
        let transient = matches!(read, Ok(Err(ConnectError::Transport { .. })));
        let Ok(backoff) = READ_BACKOFF else { return (read, now) };
        if !transient || failures == READ_RETRIES {
            return (read, now);
        }
        if let Ok(Err(e)) = &read {
            eprintln!("{PROGRAM}: an evidence read failed ({e}); reading again");
        }
        let wait = backoff.delay_ns(failures, &mut jitter);
        tokio::time::sleep(std::time::Duration::from_nanos(u64::try_from(wait).unwrap_or(0))).await;
        failures += 1;
    }
}

fn unavailable(
    path: &mut OrderPath,
    venue: &str,
    reason: &str,
    evidence: EvidenceRead,
    at_ns: i128,
) -> Result<String, Fatal> {
    eprintln!("{PROGRAM}: reconciliation {venue}: {reason}");
    path.reconciliation_unavailable(venue, reason, evidence, at_ns).map_err(unrecorded)?;
    Ok(match evidence {
        EvidenceRead::Failed => format!("{venue} unread ({reason}); resume refused"),
        _ => format!("{venue} unverified ({reason})"),
    })
}

fn verdict_name(verdict: Verdict, findings: usize) -> String {
    match verdict {
        Verdict::Aligned => "aligned".into(),
        Verdict::Unverified => format!("unverified ({findings} finding(s))"),
        Verdict::Diverged => format!("diverged ({findings} finding(s)); resume refused"),
    }
}

/// An unrecorded pass is fatal: a node that reconciled and cannot say so would resume on it.
fn unrecorded(e: bijective_control_engine::PathError) -> Fatal {
    Fatal::failure(format!("reconciliation could not be recorded: {e}"))
}
