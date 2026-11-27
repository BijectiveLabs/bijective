// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Verify a venue against its profile and record what the check concluded, in one step.
//! Usage: bijective-verify-venue <store> <venue> <profile.json> <instrument> <answer|-|url>
//! Exit codes: 0 confirmed, 1 diverged, not observed or not recorded, 2 usage.

use std::path::Path;
use std::process::ExitCode;

use bijective_common::clock::{Clock, WallClock};
use bijective_event_store::store::StreamWriter;
use bijective_venue::profile::VenueProfile;
use bijective_venue_assurance::wire::{ASSURANCE, Concluded, Outcome, record};
use bijective_venue_assurance::{AssuranceReport, NotObservedReason, Verification, verify};

/// Why no venue answer is in hand. Only an unreachable venue is a conclusion about the venue.
enum NoAnswer {
    Unreachable(String),
    Local(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [root, venue, profile, instrument, answer] = a.as_slice() else {
        eprintln!(
            "usage: verify_venue <store> <venue> <profile.json> <instrument> <answer-file|->"
        );
        return ExitCode::from(2);
    };
    let root = Path::new(root);
    let profile = match read_profile(profile, venue) {
        Ok(profile) => profile,
        Err(detail) => return fail(&detail),
    };

    let answered = match obtain_answer(venue, answer, instrument).await {
        Ok(body) => body,
        Err(NoAnswer::Local(detail)) => return fail(&detail),
        Err(NoAnswer::Unreachable(detail)) => {
            // Recorded as NotObserved so the last conclusion does not keep standing.
            eprintln!("{detail}");
            return conclude(
                root,
                venue,
                &not_observed(venue, NotObservedReason::VenueUnreachable),
            );
        }
    };

    let at_ns = WallClock::new().now_ns();
    let outcome = match bijective_venue_registry::observe_instrument(venue, &answered, instrument) {
        Ok(observed) => {
            Outcome::of(&AssuranceReport::new(venue, verify(&profile, &observed, at_ns)))
        }
        Err(e) => {
            // An unreadable answer is not a matching venue.
            eprintln!("the venue's answer could not be read: {e:?}");
            not_observed(venue, NotObservedReason::RetrievalMethodGone)
        }
    };
    conclude(root, venue, &outcome.drawn_from(answered.as_bytes()))
}

/// The profile at `path`, refused when it is unreadable or describes another venue.
fn read_profile(path: &str, venue: &str) -> Result<VenueProfile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("the profile could not be opened: {e}"))?;
    let profile = VenueProfile::from_json(&text)
        .map_err(|e| format!("the profile could not be read: {e}"))?;
    if profile.venue != venue {
        return Err(format!("the profile is for {} and the check is for {venue}", profile.venue));
    }
    Ok(profile)
}

/// The venue's instruments answer: from standard input (`-`), fetched from a URL, or read from a
/// recorded file.
async fn obtain_answer(venue: &str, answer: &str, instrument: &str) -> Result<String, NoAnswer> {
    if answer == "-" {
        return std::io::read_to_string(std::io::stdin()).map_err(|e| {
            NoAnswer::Local(format!(
                "the venue's answer could not be read from standard input: {e}"
            ))
        });
    }
    if answer.starts_with("https://") || answer.starts_with("http://") {
        return bijective_venue_registry::fetch_instrument_answer(venue, answer, instrument)
            .await
            .map_err(|e| NoAnswer::Unreachable(format!("the venue could not be reached: {e:?}")));
    }
    std::fs::read_to_string(answer)
        .map_err(|e| NoAnswer::Local(format!("the venue's answer could not be read: {e}")))
}

fn not_observed(venue: &str, reason: NotObservedReason) -> Outcome {
    let at_ns = WallClock::new().now_ns();
    Outcome::of(&AssuranceReport::new(venue, Verification::NotObserved { at_ns, reason }))
}

/// Records the outcome on the assurance stream. Success only for a recorded confirmation.
fn conclude(root: &Path, venue: &str, outcome: &Outcome) -> ExitCode {
    let recorded = StreamWriter::open(root, ASSURANCE)
        .map_err(|e| format!("the assurance stream could not be opened: {e:?}"))
        .and_then(|mut stream| {
            record(outcome, &mut stream)
                .map_err(|e| format!("the outcome could not be recorded: {e:?}"))
        });
    let at = match recorded {
        Ok(at) => at,
        Err(detail) => return fail(&detail),
    };
    let concluded = match &outcome.concluded {
        Concluded::NotObserved { .. } => "NotObserved".to_string(),
        other => format!("{other:?}"),
    };
    println!("recorded {concluded} for {venue} at position {at}");
    if outcome.is_confirmed() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn fail(detail: &str) -> ExitCode {
    eprintln!("{detail}");
    ExitCode::FAILURE
}
