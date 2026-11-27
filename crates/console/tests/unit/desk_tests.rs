// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the control desk signs, and everything it refuses to.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use bijective_event_store::store::read_range;
use bijective_serialization::signal::command::{Instruction, decode};
use bijective_serialization::signal::node::{LIVE, PAPER};

use super::*;

const TOKEN: [u8; 32] = [7; 32];

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bl-desk-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn desk(dir: &std::path::Path, role: Role) -> Desk {
    bijective_cli::keygen("ana", role, dir).expect("keygen");
    Desk::new("ana".into(), role, dir.join("ana.key"), &TOKEN).expect("desk")
}

fn form(desk: &Desk, action: &str) -> String {
    format!("token={}&action={action}", desk.token())
}

fn commands(dir: &std::path::Path) -> Vec<Instruction> {
    commands_in(dir, "commands")
}

fn commands_in(dir: &std::path::Path, stream: &str) -> Vec<Instruction> {
    read_range(dir, stream, 0, u64::MAX)
        .unwrap_or_default()
        .iter()
        .map(|e| decode(&e.payload).expect("a signed command").instruction)
        .collect()
}

#[test]
fn a_form_from_this_console_is_signed_and_appended() {
    let dir = scratch("halt");
    let desk = desk(&dir, Role::Operator);
    assert_eq!(desk.submit(&dir, LIVE, &form(&desk, "halt")), Ok(0));
    let venue = format!("{}&venue=venue-a", form(&desk, "withdraw-venue"));
    assert_eq!(desk.submit(&dir, LIVE, &venue), Ok(1));
    assert_eq!(
        commands(&dir),
        [Instruction::Halt, Instruction::WithdrawVenue { venue: "venue-a".into() }]
    );
}

#[test]
fn a_form_without_this_consoles_token_issues_nothing() {
    let dir = scratch("token");
    let desk = desk(&dir, Role::Operator);
    assert_eq!(desk.submit(&dir, LIVE, "action=halt"), Err(Refusal::NotThisConsole));
    let other = format!("token={}&action=halt", "0".repeat(64));
    assert_eq!(desk.submit(&dir, LIVE, &other), Err(Refusal::NotThisConsole));
    assert!(commands(&dir).is_empty());
}

/// The role decides what is offered. An action outside the offer is refused before signing.
#[test]
fn an_action_the_role_may_not_take_is_not_offered_and_not_signed() {
    let dir = scratch("role");
    let observer = desk(&dir, Role::Observer);
    assert!(observer.offers().contains(&Action::Halt), "stopping is open to every role");
    assert!(!observer.offers().contains(&Action::Resume));
    assert_eq!(
        observer.submit(&dir, LIVE, &form(&observer, "resume")),
        Err(Refusal::NotOffered { action: "resume".into() })
    );
    assert_eq!(
        observer.submit(&dir, LIVE, &form(&observer, "adjust-position")),
        Err(Refusal::NotOffered { action: "adjust-position".into() }),
        "position adjustments stay on the command line"
    );
    assert!(commands(&dir).is_empty());
}

#[test]
fn a_field_with_markup_or_an_oversized_form_is_refused() {
    let dir = scratch("malformed");
    let desk = desk(&dir, Role::Operator);
    let markup = format!("{}&venue=%3Cscript%3E", form(&desk, "withdraw-venue"));
    assert!(matches!(desk.submit(&dir, LIVE, &markup), Err(Refusal::Malformed { .. })));
    let long = format!("{}&venue={}", form(&desk, "halt"), "a".repeat(MAX_FORM_BYTES));
    assert!(matches!(desk.submit(&dir, LIVE, &long), Err(Refusal::Malformed { .. })));
    assert!(commands(&dir).is_empty());
}

#[test]
fn a_key_that_cannot_be_read_stops_the_desk_at_start() {
    let dir = scratch("nokey");
    let missing = Desk::new("ana".into(), Role::Operator, dir.join("absent.key"), &TOKEN);
    assert!(missing.is_err());
}

/// A console reading a paper record halts the paper node, never the live one on the same store.
#[test]
fn a_command_is_signed_for_the_node_whose_record_the_console_reads() {
    let dir = scratch("paper");
    let desk = desk(&dir, Role::Operator);
    assert_eq!(desk.submit(&dir, PAPER, &form(&desk, "halt")), Ok(0));
    assert_eq!(commands_in(&dir, "commands-paper").len(), 1);
    assert!(commands(&dir).is_empty(), "the live node's command stream is untouched");
}
