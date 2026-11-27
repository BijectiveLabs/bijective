// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the operator surface refuses before it binds.

use super::*;

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn refusal(given: Vec<String>, why: &str) -> Invalid {
    match parse_args(given.into_iter()) {
        Err(e) => e,
        Ok(_) => panic!("{why}"),
    }
}

#[test]
fn a_store_and_nothing_else_is_enough() {
    let parsed = parse_args(args("--store /s").into_iter()).expect("a minimal run was refused");
    assert_eq!(parsed.port, DEFAULT_PORT);
}

/// Binding zero picks a random port, so the printed address would not be where it listens.
#[test]
fn port_zero_is_refused_rather_than_left_to_the_system() {
    let refused = refusal(args("--store /s --port 0"), "port zero was accepted");
    assert_eq!(refused.field, "--port");
    assert!(matches!(refused.reason, Reason::OutOfRange { .. }));
}

#[test]
fn a_port_above_the_range_says_what_the_range_is() {
    let refused = refusal(args("--store /s --port 70000"), "an impossible port was accepted");
    match refused.reason {
        Reason::OutOfRange { found, maximum, .. } => {
            assert_eq!(found, 70_000);
            assert_eq!(maximum, HIGHEST_PORT);
        }
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn a_port_that_is_not_a_number_is_refused_as_unreadable() {
    let refused = refusal(args("--store /s --port seven"), "a word was taken as a port");
    assert!(matches!(refused.reason, Reason::Malformed { .. }));
}

#[test]
fn a_mistyped_flag_is_refused_with_the_flags_this_program_takes() {
    let refused = refusal(args("--store /s --prot 7333"), "a mistyped flag was accepted");
    match refused.reason {
        Reason::Unknown { ref known, .. } => {
            assert!(known.iter().any(|k| k == "--port"), "no alternatives: {known:?}");
        }
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn half_the_standing_pair_names_the_other_half() {
    let refused = refusal(args("--store /s --manifest /m.json"), "half a pair was accepted");
    match refused.reason {
        Reason::Requires { ref other } => assert_eq!(other, "--run"),
        other => panic!("refused for the wrong reason: {other:?}"),
    }

    let refused = refusal(args("--store /s --run /run"), "the other half was accepted");
    match refused.reason {
        Reason::Requires { ref other } => assert_eq!(other, "--manifest"),
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

#[test]
fn a_missing_store_is_refused_as_unset() {
    let refused = refusal(args("--port 7333"), "a surface with no store was accepted");
    assert_eq!(refused.field, "--store");
    assert!(matches!(refused.reason, Reason::Absent));
}

#[test]
fn a_flag_given_without_its_value_names_the_flag() {
    let refused = refusal(args("--store /s --port"), "a valueless flag was accepted");
    assert_eq!(refused.field, "--port");
    assert!(matches!(refused.reason, Reason::Absent));
}

/// Beyond loopback the console answers only over HTTPS with client certificates.
#[test]
fn an_address_beyond_loopback_without_tls_is_refused() {
    let refused = refusal(args("--store /s --listen 0.0.0.0"), "a plain public bind was accepted");
    assert_eq!(refused.field, "--listen");
    assert!(matches!(refused.reason, Reason::Requires { .. }));
    let loopback = parse_args(args("--store /s --listen 127.0.0.1").into_iter()).expect("loopback");
    assert!(loopback.listen.is_loopback() && loopback.tls.is_none());
}

#[test]
fn tls_needs_its_certificate_key_and_client_authority_together() {
    let refused = refusal(args("--store /s --tls-cert c.pem --tls-key k.pem"), "no client CA");
    match refused.reason {
        Reason::Requires { ref other } => assert_eq!(other, "--client-ca"),
        other => panic!("refused for the wrong reason: {other:?}"),
    }
    let whole = "--store /s --listen 0.0.0.0 --tls-cert c.pem --tls-key k.pem --client-ca ca.pem";
    assert!(parse_args(args(whole).into_iter()).expect("whole").tls.is_some());
}

#[test]
fn a_desk_needs_a_known_role_and_all_three_parts() {
    let refused = refusal(args("--store /s --operator ana --role admin --key a.key"), "admin role");
    assert_eq!(refused.field, "--role");
    let refused = refusal(args("--store /s --operator ana"), "half a desk");
    assert!(matches!(refused.reason, Reason::Requires { .. }));
}

/// The live node's record unless asked otherwise, and a mode no program uses is refused, not
/// read as live.
#[test]
fn the_run_mode_selects_the_record_and_an_unknown_one_is_refused() {
    let live = parse_args(args("--store /s").into_iter()).expect("live by default");
    assert_eq!(live.node, bijective_serialization::signal::node::LIVE);
    let paper = parse_args(args("--store /s --mode paper").into_iter()).expect("paper");
    assert_eq!(paper.node.control, "control-paper");
    let refused = refusal(args("--store /s --mode shadow"), "an unknown mode was accepted");
    assert_eq!(refused.field, "--mode");
}
