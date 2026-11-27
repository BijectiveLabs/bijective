// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operator's command line: keys, and signed commands appended to the stream.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use bijective_authority::Role;
use bijective_event_store::store::read_range;
use bijective_serialization::signal::Instruction;
use bijective_serialization::signal::command::decode_signed;

use super::{issue, keygen, parse_instruction};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bijective-cli-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

#[test]
fn keygen_writes_a_key_and_returns_the_registry_entry_for_its_public_half() {
    let dir = scratch("keygen");
    let entry = keygen("ada", Role::Operator, &dir).expect("generated");
    assert!(entry.contains(r#""name": "ada""#) && entry.contains(r#""role": "operator""#));
    let seed = std::fs::read_to_string(dir.join("ada.key")).expect("key file");
    assert_eq!(seed.trim().len(), 64);
    assert!(!entry.contains(seed.trim()), "the entry must not carry the private half");
}

#[test]
fn keygen_will_not_overwrite_an_existing_key() {
    let dir = scratch("overwrite");
    keygen("ada", Role::Operator, &dir).expect("first");
    assert!(keygen("ada", Role::Operator, &dir).is_err());
}

#[test]
fn an_issued_command_lands_in_the_stream_signed() {
    let dir = scratch("issue");
    keygen("ada", Role::Operator, &dir).expect("key");
    let instruction = parse_instruction("resume", None, &[]).expect("instruction");
    let key = dir.join("ada.key");
    let live = bijective_serialization::signal::node::LIVE;
    let position = issue(&dir, ("ada", Role::Operator, &key), instruction, live).expect("issued");

    let entries = read_range(&dir, "commands", 0, u64::MAX).expect("stream");
    assert_eq!(entries.len(), 1);
    let (command, signature) = decode_signed(&entries[0].payload).expect("decode");
    assert_eq!((command.actor.as_str(), position), ("ada", entries[0].seq));
    assert!(signature.is_some(), "a command issued here is always signed");
}

#[test]
fn an_action_that_needs_a_venue_is_refused_without_one() {
    assert!(parse_instruction("withdraw-venue", None, &[]).is_err());
    assert_eq!(
        parse_instruction("withdraw-venue", Some("venue-a".into()), &[]).expect("parsed"),
        Instruction::WithdrawVenue { venue: "venue-a".into() }
    );
    assert!(parse_instruction("launch", None, &[]).is_err());
}

#[test]
fn a_position_adjustment_takes_scaled_integers_and_a_reason() {
    let rest: Vec<String> =
        ["AAA", "buy", "7", "1000", "held", "before"].iter().map(|s| (*s).to_string()).collect();
    let parsed = parse_instruction("adjust-position", Some("venue-a".into()), &rest).expect("ok");
    let Instruction::AdjustPosition { adjustment, .. } = parsed else {
        panic!("not an adjustment")
    };
    assert_eq!((adjustment.quantity_scaled, adjustment.price_scaled), (7, 1_000));
    assert_eq!(adjustment.reason, "held before");
    assert!(parse_instruction("adjust-position", Some("v".into()), &["AAA".to_string()]).is_err());
}

/// The owner comes before the reason. A strategy alone names no owner and is refused.
#[test]
fn a_position_adjustment_may_name_its_desk_and_strategy() {
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_string()).collect::<Vec<String>>();
    let rest = words(&[
        "AAA",
        "buy",
        "7",
        "1000",
        "--desk",
        "alpha",
        "--strategy",
        "trend",
        "moved",
        "in",
    ]);
    let parsed = parse_instruction("adjust-position", Some("venue-a".into()), &rest).expect("ok");
    let Instruction::AdjustPosition { adjustment, .. } = parsed else {
        panic!("not an adjustment")
    };
    assert_eq!(
        adjustment.owner,
        Some(bijective_model::Owner { desk: "alpha".into(), strategy: Some("trend".into()) })
    );
    assert_eq!(adjustment.reason, "moved in");

    let strategy_only = words(&["AAA", "buy", "7", "1000", "--strategy", "trend", "moved"]);
    assert!(parse_instruction("adjust-position", Some("v".into()), &strategy_only).is_err());
}
