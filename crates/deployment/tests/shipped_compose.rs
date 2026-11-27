// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The container composition is derived from the shipped manifest, never written beside it.

use bijective_deployment::{Deployment, compose};

const SHIPPED: &str = include_str!("../deployment.json");

fn composed() -> String {
    compose(&Deployment::parse(SHIPPED.as_bytes()).expect("the shipped manifest is sound"))
}

/// One service's block, from its name to the blank line that ends it.
fn section(text: &str, name: &str) -> String {
    let from = text.find(&format!("\n  {name}:\n")).expect("the service is composed");
    text[from + 1..].split("\n\n").next().expect("a block").to_string()
}

#[test]
fn the_composition_is_the_same_bytes_every_time() {
    assert_eq!(composed(), composed());
}

#[test]
fn every_program_is_a_service_that_runs_its_manifest_binary() {
    let text = composed();
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();
    for program in &d.programs {
        let block = section(&text, &program.name);
        assert!(
            block.contains(&format!("entrypoint: [{:?}]", program.binary)),
            "{} does not run {}",
            program.name,
            program.binary
        );
    }
}

/// The unit's confinement, carried over: read-only root, no new privileges, no capability, and
/// the memory bound and stop timeout of the manifest.
#[test]
fn a_service_is_confined_as_its_unit_is() {
    let text = composed();
    assert_eq!(text.matches("read_only: true").count(), 4);
    assert_eq!(text.matches("no-new-privileges:true").count(), 4);
    assert_eq!(text.matches("cap_drop:\n      - ALL").count(), 4);
    assert!(text.contains("mem_limit: 1024m"));
    assert!(text.contains("stop_grace_period: 60s"));
}

/// The core runs alone: only the operational side waits for a profile to be asked for.
#[test]
fn only_the_operational_programs_need_a_profile() {
    let text = composed();
    assert!(!section(&text, "data-engine").contains("profiles:"));
    assert!(!section(&text, "control-engine").contains("profiles:"));
    assert!(section(&text, "operational-host").contains("profiles: [\"operational\"]"));
    assert!(section(&text, "strategy-engine").contains("profiles: [\"operational\"]"));
}

#[test]
fn start_order_and_credentials_follow_the_manifest() {
    let text = composed();
    let control = section(&text, "control-engine");
    assert!(control.contains("data-engine:\n        condition: service_started"));
    assert!(control.contains("secrets:\n      - bijective-init"));
    assert!(control.contains("CREDENTIALS_DIRECTORY: /run/secrets"));
    assert!(text.contains(
        "secrets:\n  bijective-init:\n    file: \"/etc/bijective/credentials/bijective-init\""
    ));
}

#[test]
fn a_writable_path_is_a_named_volume_shared_by_the_programs_that_declare_it() {
    let text = composed();
    assert!(text.contains("- var-lib-bijective-store:/var/lib/bijective/store"));
    assert!(text.contains("- run-bijective:/run/bijective"));
    assert_eq!(text.matches("\n  var-lib-bijective-store: {}").count(), 1);
}
