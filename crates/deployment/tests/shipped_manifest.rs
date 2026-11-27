// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The shipped manifest must validate and generate reproducibly.
//! `include_str!` binds the test to the file, not to a copy.

use bijective_deployment::{Deployment, ProgramClass, generate};

const SHIPPED: &str = include_str!("../deployment.json");

#[test]
fn the_shipped_manifest_validates() {
    Deployment::parse(SHIPPED.as_bytes()).expect("the shipped deployment manifest is sound");
}

#[test]
fn the_shipped_manifest_generates_reproducibly() {
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();
    assert_eq!(generate(&d).concatenated(), generate(&d).concatenated());
}

/// The core-runs-alone rule in the artefact: the core is enabled at install and the operational side is not.
#[test]
fn the_shipped_manifest_leaves_the_operational_side_stopped() {
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();
    let generated = generate(&d);

    let operational: Vec<&str> = d
        .programs
        .iter()
        .filter(|p| p.class == ProgramClass::Operational)
        .map(|p| p.name.as_str())
        .collect();
    assert!(
        !operational.is_empty(),
        "an artefact with nothing switchable cannot demonstrate that the core runs alone"
    );

    for name in operational {
        let unit = format!("bijective-{name}.service");
        assert!(generated.units.contains_key(&unit), "{unit} is not installed");
        assert!(!generated.enable.contains(&unit), "{unit} is enabled at install");
    }
}

/// Every program the manifest installs must be a binary this build produces.
#[test]
fn every_program_named_is_one_this_build_produces() {
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();
    // Read off the crate manifests: a hard-coded list goes stale and the test still passes.
    let declared = binaries_this_workspace_declares();
    assert!(!declared.is_empty(), "no crate manifest was read, so this proves nothing");

    for p in &d.programs {
        let file = p.binary.rsplit('/').next().unwrap_or(&p.binary);
        assert!(
            declared.iter().any(|d| d == file),
            "{file} is named in the manifest and no crate here declares it"
        );
    }
}

/// Every `[[bin]]` name the crates in this workspace declare.
fn binaries_this_workspace_declares() -> Vec<String> {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, found);
            } else if path.file_name().is_some_and(|n| n == "Cargo.toml") {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let mut in_bin = false;
                for line in text.lines() {
                    let line = line.trim();
                    if line.starts_with('[') {
                        in_bin = line == "[[bin]]";
                    } else if in_bin
                        && let Some(rest) = line.strip_prefix("name = \"")
                        && let Some(name) = rest.strip_suffix('"')
                    {
                        found.push(name.to_string());
                    }
                }
            }
        }
    }

    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(std::path::Path::to_path_buf)
        .expect("the core directory");
    let mut found = Vec::new();
    walk(&core, &mut found);
    found
}

/// The kit ships the assurance machinery and enables none of it: which venue to check is an operator's decision.
#[test]
fn the_shipped_manifest_installs_the_check_and_schedules_nothing() {
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();
    let generated = generate(&d);

    let check = d.checks.first().expect("the kit ships a scheduled check");
    assert!(check.instances.is_empty(), "the kit chooses no venue for a Client");
    assert!(generated.units.contains_key(&format!("bijective-{}@.service", check.name)));
    assert!(generated.units.contains_key(&format!("bijective-{}@.timer", check.name)));
    assert!(
        !generated.enable.iter().any(|u| u.contains('@')),
        "nothing instanced is enabled at install: {:?}",
        generated.enable
    );
}

/// Every binary the manifest names is one this build produces, checks included.
#[test]
fn every_binary_named_by_a_check_is_one_this_build_produces() {
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();

    for check in &d.checks {
        let name = check.binary.rsplit('/').next().expect("a binary path");
        assert_eq!(
            name, "bijective-verify-venue",
            "a check naming a binary this build does not produce installs a timer that fails"
        );
    }
}

/// The control engine has no default mode: a node that names none refuses to start, so the shipped
/// artefact must name the one it runs in. The installed node trades, so that mode is live.
#[test]
fn the_shipped_control_engine_names_its_mode() {
    let d = Deployment::parse(SHIPPED.as_bytes()).unwrap();
    let engine = d
        .programs
        .iter()
        .find(|p| p.name == "control-engine")
        .expect("the manifest carries the control engine");

    let named: Vec<&str> = engine
        .arguments
        .windows(2)
        .filter(|pair| pair[0] == "--mode")
        .map(|pair| pair[1].as_str())
        .collect();
    assert_eq!(named, ["live"], "{:?}", engine.arguments);
}
