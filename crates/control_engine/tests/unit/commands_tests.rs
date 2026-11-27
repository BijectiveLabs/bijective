// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Operator commands on the order path: who may be believed, and a signed command used once.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use bijective_authority::Role;
use bijective_event_store::store::read_range;
use bijective_identity::{Operator, OperatorRegistry, SigningKey};
use bijective_limits::SafeState;
use bijective_serialization::signal::command::{Command, Instruction, encode};

use super::OrderPath;
use crate::wire::{ControlEvent, decode};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bijective-commands-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch root");
    dir
}

fn key(seed_byte: u8) -> SigningKey {
    SigningKey::from_seed(&[seed_byte; 32]).expect("seed")
}

fn registry() -> OperatorRegistry {
    OperatorRegistry::new([
        ("ada".to_string(), Operator { role: Role::Operator, public_key: key(1).public_key() }),
        ("bo".to_string(), Operator { role: Role::Observer, public_key: key(2).public_key() }),
    ])
    .expect("registry")
}

fn command(id: &str, actor: &str, role: Role, instruction: Instruction) -> Command {
    Command { id: id.into(), actor: actor.into(), role, instruction, at_ns: 1 }
}

fn signed_by(key: &SigningKey, command: &Command) -> [u8; 64] {
    key.sign(&encode(command).expect("encode"))
}

fn open(root: &std::path::Path) -> OrderPath {
    let mut path = OrderPath::open(root, "control", "limits").expect("open");
    path.set_operators(registry());
    path
}

fn details(root: &std::path::Path) -> Vec<String> {
    read_range(root, "control", 0, u64::MAX)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match decode(&e.payload).expect("decode") {
            ControlEvent::CommandApplied { detail, .. } => Some(detail),
            _ => None,
        })
        .collect()
}

#[test]
fn a_command_signed_by_its_registered_operator_is_carried_out() {
    let r = root("accepted");
    let mut path = open(&r);
    let resume = command("c-1", "ada", Role::Operator, Instruction::Resume);
    let signature = signed_by(&key(1), &resume);
    let state = path.carry_out(&resume, Some(&signature), 0, 10).expect("carried");
    assert_eq!(state, SafeState::Active);
}

#[test]
fn an_unsigned_command_is_refused_and_the_refusal_is_recorded() {
    let r = root("unsigned");
    let mut path = open(&r);
    let resume = command("c-1", "ada", Role::Operator, Instruction::Resume);
    let state = path.carry_out(&resume, None, 0, 10).expect("recorded");
    assert_eq!(state, SafeState::Halted, "an unsigned resume must not open the platform");
    drop(path);
    assert!(details(&r)[0].contains("no signature"), "{:?}", details(&r));
}

#[test]
fn a_signature_by_someone_elses_key_is_refused() {
    let r = root("wrong-key");
    let mut path = open(&r);
    let resume = command("c-1", "ada", Role::Operator, Instruction::Resume);
    let signature = signed_by(&key(2), &resume);
    let state = path.carry_out(&resume, Some(&signature), 0, 10).expect("recorded");
    assert_eq!(state, SafeState::Halted);
}

/// A registered observer cannot act as an operator by saying so. The role is the registry's.
#[test]
fn a_claimed_role_above_the_registered_one_is_refused() {
    let r = root("role");
    let mut path = open(&r);
    let resume = command("c-1", "bo", Role::Operator, Instruction::Resume);
    let signature = signed_by(&key(2), &resume);
    let state = path.carry_out(&resume, Some(&signature), 0, 10).expect("recorded");
    assert_eq!(state, SafeState::Halted);
    drop(path);
    assert!(details(&r)[0].contains("claims the operator role"), "{:?}", details(&r));
}

/// A signed command that is appended to the stream again is the same command. Without this, a
/// recorded resume would reopen the platform each time someone with write access replayed it.
#[test]
fn a_signed_command_is_carried_out_once() {
    let r = root("replay");
    let mut path = open(&r);
    let resume = command("c-1", "ada", Role::Operator, Instruction::Resume);
    let resume_signature = signed_by(&key(1), &resume);
    path.carry_out(&resume, Some(&resume_signature), 0, 10).expect("first");

    let halt = command("c-2", "ada", Role::Operator, Instruction::Halt);
    let halt_signature = signed_by(&key(1), &halt);
    assert_eq!(
        path.carry_out(&halt, Some(&halt_signature), 1, 11).expect("halt"),
        SafeState::Halted
    );

    let again = path.carry_out(&resume, Some(&resume_signature), 2, 12).expect("recorded");
    assert_eq!(again, SafeState::Halted, "the same resume must not reopen the platform");
    drop(path);
    assert!(details(&r).last().expect("a record").contains("already"), "{:?}", details(&r));
}

/// With no registry the path believes the claimed role, as it always has. The node refuses to
/// start in live mode without one.
#[test]
fn without_a_registry_the_claimed_role_stands() {
    let r = root("open");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    let resume = command("c-1", "ada", Role::Operator, Instruction::Resume);
    assert_eq!(path.carry_out(&resume, None, 0, 10).expect("carried"), SafeState::Active);
}

/// A flatten leaves the node reducing. An observer's is refused and carried out by nobody, which
/// the node reads before it routes or flattens anything.
#[test]
fn a_flatten_reduces_and_an_observers_is_refused_and_not_carried_out() {
    let r = root("flatten");
    let mut path = open(&r);
    path.resume(SafeState::Active);

    let observers = command("c-1", "bo", Role::Observer, Instruction::Flatten);
    let state =
        path.carry_out(&observers, Some(&signed_by(&key(2), &observers)), 0, 1).expect("recorded");
    assert_eq!(state, SafeState::Active);
    assert!(!path.last_carried_out());

    let operators = command("c-2", "ada", Role::Operator, Instruction::Flatten);
    let state =
        path.carry_out(&operators, Some(&signed_by(&key(1), &operators)), 1, 2).expect("recorded");
    assert_eq!(state, SafeState::Reducing);
    assert!(path.last_carried_out());
    assert_eq!(path.recorded_orders().and_then(|o| o.flatten_asked()), Some("c-2"));
}

/// An unsigned venue admission under a registry is refused, so the node must not admit the venue.
#[test]
fn an_unsigned_command_is_not_carried_out() {
    let r = root("unsigned-admit");
    let mut path = open(&r);
    let admit =
        command("c-3", "ada", Role::Operator, Instruction::AdmitVenue { venue: "venue-a".into() });
    path.carry_out(&admit, None, 0, 1).expect("recorded");
    assert!(!path.last_carried_out());
}
