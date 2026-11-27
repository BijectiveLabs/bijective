// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A command issued by the command line is believed by the control engine only for its operator.

use std::path::PathBuf;

use bijective_authority::Role;
use bijective_cli::{issue, keygen, parse_instruction};
use bijective_common::clock::SequenceClock;
use bijective_control_engine::{OrderPath, carry_out_commands, parse_operators};
use bijective_limits::SafeState;
use bijective_serialization::signal::node::{LIVE, PAPER};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-cli-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// Both operators are registered. Only Ada's key signs. A command Bo issues with Ada's name is
/// refused, one Ada issues is carried out, and the platform's state follows only the second.
#[tokio::test]
async fn only_the_named_operators_key_opens_the_platform() {
    let dir = scratch("believed");
    let ada = keygen("ada", Role::Operator, &dir).expect("ada");
    let bo = keygen("bo", Role::Operator, &dir).expect("bo");
    let registry =
        parse_operators(format!(r#"{{ "version": 1, "operators": [ {ada}, {bo} ] }}"#).as_bytes())
            .expect("registry");

    let mut path = OrderPath::open(&dir, "control", "limits").expect("open");
    path.set_operators(registry);
    let resume = || parse_instruction("resume", None, &[]).expect("instruction");

    // Bo's key under Ada's name.
    let key = dir.join("bo.key");
    issue(&dir, ("ada", Role::Operator, &key), resume(), LIVE).expect("issued");
    let clock = SequenceClock::starting_at(1_000);
    let carried = carry_out_commands(&mut path, &clock, &dir, "commands").expect("carried");
    assert_eq!(carried.last().expect("a command").state, SafeState::Halted);

    let key = dir.join("ada.key");
    issue(&dir, ("ada", Role::Operator, &key), resume(), LIVE).expect("issued");
    let carried = carry_out_commands(&mut path, &clock, &dir, "commands").expect("carried");
    assert_eq!(carried.last().expect("a command").state, SafeState::Active);
}

/// A paper command lands on the paper node's stream: the live node never carries it out.
#[tokio::test]
async fn a_paper_command_never_reaches_the_live_node() {
    let dir = scratch("paper");
    let ada = keygen("ada", Role::Operator, &dir).expect("ada");
    let registry =
        parse_operators(format!(r#"{{ "version": 1, "operators": [ {ada} ] }}"#).as_bytes())
            .expect("registry");
    let mut live = OrderPath::open(&dir, LIVE.control, LIVE.limits).expect("open");
    live.set_operators(registry);
    let resume = parse_instruction("resume", None, &[]).expect("instruction");

    issue(&dir, ("ada", Role::Operator, &dir.join("ada.key")), resume, PAPER).expect("issued");
    let clock = SequenceClock::starting_at(1_000);
    let carried = carry_out_commands(&mut live, &clock, &dir, LIVE.commands).expect("carried");
    assert!(carried.is_empty(), "the live node read a paper command: {carried:?}");
}
