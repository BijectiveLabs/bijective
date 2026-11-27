// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The control offer names what the operator's role may take, and nothing without an operator.

use bijective_authority::Role;

use super::*;

#[test]
fn without_an_operator_nothing_is_offered() {
    let document: serde_json::Value =
        serde_json::from_str(&offer(None, "paper", &BTreeSet::new())).expect("json");
    assert_eq!(document["issuing"], false);
    assert_eq!(document["mode"], "paper");
    assert!(document.get("token").is_none() && document.get("actions").is_none());
}

#[test]
fn an_operator_is_offered_its_role_s_actions_with_the_form_token() {
    let dir = std::env::temp_dir().join(format!("bl-control-offer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    bijective_cli::keygen("ana", Role::Operator, &dir).expect("keygen");
    let desk =
        Desk::new("ana".into(), Role::Operator, dir.join("ana.key"), &[7; 32]).expect("desk");
    let venues: BTreeSet<String> = ["venue-a".to_string()].into();
    let document: serde_json::Value =
        serde_json::from_str(&offer(Some(&desk), "live", &venues)).expect("json");
    assert_eq!(document["issuing"], true);
    assert_eq!(document["operator"], "ana");
    assert_eq!(document["token"], desk.token());
    let actions: Vec<&str> = document["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .filter_map(|a| a.as_str())
        .collect();
    let offered: Vec<&str> = desk.offers().iter().map(|a| a.as_str()).collect();
    assert_eq!(actions, offered);
    assert_eq!(document["venues"], serde_json::json!(["venue-a"]));
}
