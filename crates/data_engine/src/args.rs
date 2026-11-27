// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the program is asked to do, checked before it opens anything.

use std::path::PathBuf;

use bijective_common::configuration::{Invalid, Reason, distinct, known, non_empty, required};

/// A list so a mistyped flag is refused with the alternatives (configuration assurance `Unknown`).
pub(crate) const FLAGS: &[&str] = &[
    "--store",
    "--readiness",
    "--venue",
    "--profile",
    "--recording",
    "--replay-store",
    "--archive",
    "--endpoint",
    "--instruments",
    "--drill-seconds",
    "--journal-segment-bytes",
    "--keystore",
    "--credential",
    "--key-name",
    "--secret-name",
    "--stream-name",
];

pub(crate) const ARGUMENT: &str = "argument";

/// Flags decision 0007 retired: precision is read per instrument from `--profile`.
const RETIRED: &[&str] = &["--price-scale", "--quantity-scale"];

/// Where the account credential of a stream that needs one is read: the broker's keystore, opened
/// with the init credential, and the names its two parts are stored under. Never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    pub(crate) keystore: PathBuf,
    pub(crate) credential: String,
    pub(crate) key_name: String,
    pub(crate) secret_name: String,
}

/// The same normalisation runs behind both, so a drill and a production run are the same run.
pub(crate) enum Source {
    /// Frames as a recorder wrote them, one JSON line each with its arrival time.
    Recording(PathBuf),
    /// Another store's frame journal for this venue, replayed entry for entry.
    Journal(PathBuf),
    Live {
        endpoint: String,
        instruments: Vec<String>,
    },
}

pub(crate) struct Args {
    pub(crate) store_root: PathBuf,
    pub(crate) readiness: PathBuf,
    pub(crate) venue: String,
    /// Required: the identity a book update is written under comes from its catalogue.
    pub(crate) profile: PathBuf,
    pub(crate) source: Source,
    pub(crate) drill_seconds: Option<u64>,
    /// The journal's rotation bound. Retention removes whole segments, so this is its granularity.
    pub(crate) journal_segment_bytes: Option<u64>,
    /// Where market records are archived. Absent, they are counted and reported unarchived.
    pub(crate) archive: Option<PathBuf>,
    pub(crate) account: Option<Account>,
    /// What the market, facts, journal, link and archive streams are named after: the venue,
    /// unless a second feed of one venue (another endpoint, another price scale) needs its own.
    pub(crate) stream_name: String,
}

/// Split here, not where the flag is read: `"".split(',')` yields one empty name, which once passed the emptiness check.
fn instrument_list(raw: &str) -> Result<Vec<String>, Invalid> {
    let names: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).collect();
    for (position, name) in names.iter().enumerate() {
        if name.is_empty() {
            return Err(Invalid::new(
                "--instruments",
                Reason::Malformed { detail: format!("names nothing at position {}", position + 1) },
            ));
        }
    }
    non_empty("--instruments", &names)?;
    // A repeated name would subscribe twice and write its frames twice under one identity.
    distinct("--instruments", &names)?;
    Ok(names)
}

/// Flag values as given, before any is checked against another.
#[derive(Default)]
struct Given {
    store_root: Option<PathBuf>,
    readiness: Option<PathBuf>,
    profile: Option<PathBuf>,
    venue: Option<String>,
    recording: Option<PathBuf>,
    replay_store: Option<PathBuf>,
    endpoint: Option<String>,
    instruments: Option<String>,
    drill_seconds: Option<u64>,
    journal_segment_bytes: Option<u64>,
    archive: Option<PathBuf>,
    keystore: Option<PathBuf>,
    credential: Option<String>,
    key_name: Option<String>,
    secret_name: Option<String>,
    stream_name: Option<String>,
}

/// All four account flags or none: a partial set names a credential that cannot be read.
fn account_of(given: &mut Given) -> Result<Option<Account>, Invalid> {
    match (
        given.keystore.take(),
        given.credential.take(),
        given.key_name.take(),
        given.secret_name.take(),
    ) {
        (None, None, None, None) => Ok(None),
        (Some(keystore), Some(credential), Some(key_name), Some(secret_name)) => {
            Ok(Some(Account { keystore, credential, key_name, secret_name }))
        }
        _ => Err(Invalid::new(
            "--keystore",
            Reason::Requires { other: "--credential, --key-name and --secret-name".to_string() },
        )),
    }
}

/// Takes the arguments rather than reading them, so every refusal is reachable from a test.
pub(crate) fn parse_args<I: Iterator<Item = String>>(given: I) -> Result<Args, Invalid> {
    let mut given = read_flags(given)?;
    let account = account_of(&mut given)?;
    let source = source_of(given.recording, given.replay_store, given.endpoint, given.instruments)?;
    // Resolved before the store opens, else a bad name leaves a stream directory in the durable record.
    let venue = required("--venue", given.venue)?;
    known("--venue", &venue, bijective_venue_registry::REGISTERED)?;
    let stream_name = stream_name_of(given.stream_name, &venue)?;
    Ok(Args {
        store_root: required("--store", given.store_root)?,
        readiness: required("--readiness", given.readiness)?,
        venue,
        profile: required("--profile", given.profile)?,
        source,
        drill_seconds: given.drill_seconds,
        journal_segment_bytes: given.journal_segment_bytes,
        archive: given.archive,
        account,
        stream_name,
    })
}

/// The store admits only letters, digits, `-` and `_` in a stream name. Refused here, before a
/// directory is made.
fn stream_name_of(given: Option<String>, venue: &str) -> Result<String, Invalid> {
    let Some(name) = given else { return Ok(venue.to_string()) };
    let admitted = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if name.is_empty() || !name.chars().all(admitted) {
        let detail = format!("{name:?} holds a character other than a letter, digit, - or _");
        return Err(Invalid::new("--stream-name", Reason::Malformed { detail }));
    }
    Ok(name)
}

fn read_flags<I: Iterator<Item = String>>(mut it: I) -> Result<Given, Invalid> {
    let mut given = Given::default();
    while let Some(flag) = it.next() {
        if RETIRED.contains(&flag.as_str()) {
            let detail = "precision is read per instrument from --profile (decision 0007); \
                          remove this flag"
                .to_string();
            return Err(Invalid::new(&flag, Reason::Malformed { detail }));
        }
        // Checked before matching so a mistyped flag is refused with every known flag.
        known(ARGUMENT, &flag, FLAGS)?;
        let value = it.next().ok_or_else(|| Invalid::new(&flag, Reason::Absent))?;
        match flag.as_str() {
            "--store" => given.store_root = Some(PathBuf::from(value)),
            "--readiness" => given.readiness = Some(PathBuf::from(value)),
            "--venue" => given.venue = Some(value),
            "--profile" => given.profile = Some(PathBuf::from(value)),
            "--recording" => given.recording = Some(PathBuf::from(value)),
            "--replay-store" => given.replay_store = Some(PathBuf::from(value)),
            "--archive" => given.archive = Some(PathBuf::from(value)),
            "--keystore" => given.keystore = Some(PathBuf::from(value)),
            "--credential" => given.credential = Some(value),
            "--key-name" => given.key_name = Some(value),
            "--secret-name" => given.secret_name = Some(value),
            "--stream-name" => given.stream_name = Some(value),
            "--endpoint" => given.endpoint = Some(value),
            "--instruments" => given.instruments = Some(value),
            "--drill-seconds" => given.drill_seconds = Some(whole(&flag, &value, "seconds")?),
            "--journal-segment-bytes" => {
                given.journal_segment_bytes = Some(whole(&flag, &value, "bytes")?);
            }
            // Unreachable after `known`. Refused rather than panicked, as the process holds the store's writer lock.
            other => {
                return Err(Invalid::new(
                    ARGUMENT,
                    Reason::Malformed {
                        detail: format!("{other} is listed as an argument and has no handler"),
                    },
                ));
            }
        }
    }
    Ok(given)
}

/// A flag value that must be a whole number of `unit`.
fn whole<T: std::str::FromStr>(flag: &str, raw: &str, unit: &str) -> Result<T, Invalid> {
    raw.parse().map_err(|_| {
        Invalid::new(
            flag,
            Reason::Malformed { detail: format!("{raw} is not a whole number of {unit}") },
        )
    })
}

/// Exactly one source and no default: a data engine with no source is a misconfiguration.
fn source_of(
    recording: Option<PathBuf>,
    replay_store: Option<PathBuf>,
    endpoint: Option<String>,
    instruments: Option<String>,
) -> Result<Source, Invalid> {
    let given: Vec<&str> = [
        recording.as_ref().map(|_| "--recording"),
        replay_store.as_ref().map(|_| "--replay-store"),
        endpoint.as_ref().map(|_| "--endpoint"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if given.len() > 1 {
        return Err(Invalid::new(given[0], Reason::Conflicting { with: given[1].to_string() }));
    }
    if let Some(path) = recording {
        return Ok(Source::Recording(path));
    }
    if let Some(path) = replay_store {
        return Ok(Source::Journal(path));
    }
    let Some(endpoint) = endpoint else {
        return Err(Invalid::new("--recording, --replay-store or --endpoint", Reason::Absent));
    };
    let raw = instruments.ok_or_else(|| {
        Invalid::new("--endpoint", Reason::Requires { other: "--instruments".to_string() })
    })?;
    Ok(Source::Live { endpoint, instruments: instrument_list(&raw)? })
}

#[cfg(test)]
#[path = "../tests/unit/arguments_tests.rs"]
mod tests;
