// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What this program refuses before it opens anything.

use super::*;

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

/// Taken from the registry: the adapter contract keeps venue names in the adapters.
fn registered() -> &'static str {
    bijective_venue_registry::REGISTERED[0]
}

fn respelled() -> String {
    let name = registered();
    let mut respelled = name[..1].to_ascii_uppercase();
    respelled.push_str(&name[1..]);
    respelled
}

fn recording_run() -> Vec<String> {
    let mut given = args("--store /s --readiness /r.json --profile /p.json --recording /rec.jsonl");
    given.push("--venue".to_string());
    given.push(registered().to_string());
    given
}

/// `expect_err` would need `Debug` on `Args`. No derive is added for a test's convenience.
fn refusal(given: Vec<String>, why: &str) -> Invalid {
    match parse_args(given.into_iter()) {
        Err(e) => e,
        Ok(_) => panic!("{why}"),
    }
}

fn live_run(instruments: &str) -> Vec<String> {
    let mut given = args("--store /s --readiness /r.json --profile /p.json --endpoint wss://v");
    given.push("--venue".to_string());
    given.push(registered().to_string());
    given.push("--instruments".to_string());
    given.push(instruments.to_string());
    given
}

#[test]
fn a_complete_recording_run_is_accepted() {
    let parsed = parse_args(recording_run().into_iter()).expect("a complete run was refused");
    assert_eq!(parsed.venue, registered());
}

/// `"".split(',')` yields one empty name, which passed the old emptiness check.
#[test]
fn an_empty_instrument_list_is_refused() {
    let refusal = refusal(live_run(""), "an empty list was accepted");
    assert_eq!(refusal.field, "--instruments");
}

#[test]
fn a_blank_name_inside_the_list_is_refused_with_its_position() {
    let refusal = refusal(live_run("BTCUSDT,,ETHUSDT"), "a blank name was taken");
    match refusal.reason {
        Reason::Malformed { ref detail } => {
            assert!(detail.contains('2'), "the refusal does not say where: {detail}");
        }
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn a_repeated_instrument_is_refused() {
    let refusal = refusal(live_run("BTCUSDT,BTCUSDT"), "a repeat was accepted");
    assert!(matches!(refusal.reason, Reason::Duplicate { .. }));
}

/// Everything downstream uses this string as an identity, and the store opened before the registry was asked.
#[test]
fn a_venue_spelled_differently_is_refused_with_the_registered_spelling() {
    // Needs a registry name with a letter to change, else it proves nothing about spellings.
    assert_ne!(respelled(), registered(), "the two spellings are the same string");
    let mut given = recording_run();
    let last = given.len() - 1;
    given[last] = respelled();
    let refusal = refusal(given, "a second spelling was accepted");
    assert_eq!(refusal.field, "--venue");
    match refusal.reason {
        Reason::Unknown { ref known, .. } => {
            assert!(known.iter().any(|k| k == registered()), "offers nothing: {known:?}");
        }
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn a_mistyped_flag_is_refused_with_the_flags_this_program_takes() {
    let mut given = recording_run();
    given.push("--stroe".to_string());
    let refusal = refusal(given, "a mistyped flag was accepted");
    match refusal.reason {
        Reason::Unknown { ref known, .. } => {
            assert!(known.iter().any(|k| k == "--store"), "no alternatives: {known:?}");
        }
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn two_sources_are_refused_naming_the_other_one() {
    let mut given = recording_run();
    given.push("--endpoint".to_string());
    given.push("wss://v".to_string());
    let refusal = refusal(given, "two sources were accepted");
    match refusal.reason {
        Reason::Conflicting { ref with } => assert_eq!(with, "--endpoint"),
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn a_live_source_without_instruments_names_what_it_needs() {
    let mut given = args("--store /s --readiness /r.json --profile /p.json --endpoint wss://v");
    given.push("--venue".to_string());
    given.push(registered().to_string());
    let refusal = refusal(given, "a live run with no instruments ran");
    match refusal.reason {
        Reason::Requires { ref other } => assert_eq!(other, "--instruments"),
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn no_source_at_all_is_refused_rather_than_idled() {
    let mut given = args("--store /s --readiness /r.json --profile /p.json");
    given.push("--venue".to_string());
    given.push(registered().to_string());
    let refusal = refusal(given, "a run with no source was accepted");
    assert!(matches!(refusal.reason, Reason::Absent));
}

#[test]
fn a_flag_given_without_its_value_names_the_flag() {
    let mut given = recording_run();
    given.push("--drill-seconds".to_string());
    let refusal = refusal(given, "a valueless flag was accepted");
    assert_eq!(refusal.field, "--drill-seconds");
    assert!(matches!(refusal.reason, Reason::Absent));
}

#[test]
fn streams_are_named_after_the_venue_unless_a_stream_name_is_given() {
    let Ok(plain) = parse_args(live_run("AAA").into_iter()) else { panic!("accepted") };
    assert_eq!(plain.stream_name, registered());

    let mut named = live_run("AAA");
    named.extend(["--stream-name".to_string(), "venue-crypto".to_string()]);
    let Ok(named) = parse_args(named.into_iter()) else { panic!("accepted") };
    assert_eq!(named.stream_name, "venue-crypto");

    let mut bad = live_run("AAA");
    bad.extend(["--stream-name".to_string(), "a.b".to_string()]);
    assert_eq!(refusal(bad, "a dot was admitted").field, "--stream-name");
}
