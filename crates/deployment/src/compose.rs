// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Manifest to a container composition, deterministically.
//!
//! The same manifest that produces the init-system units produces this file, so the two cannot
//! describe different nodes. The confinement the units state is stated again here: a read-only
//! root, no new privileges, no capability, a memory bound and a stop timeout. Strings are written
//! as JSON, which YAML reads as written.

use std::collections::{BTreeMap, BTreeSet};

use crate::manifest::{Deployment, Program, ProgramClass};

const CREDENTIALS_DIRECTORY: &str = "/run/secrets";

fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// A volume name from the whole path, so two paths never share one:
/// `/var/lib/bijective/store` becomes `var-lib-bijective-store`.
fn volume_name(path: &str) -> String {
    path.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

/// `name:path` as the manifest declares a credential.
fn credential(entry: &str) -> (&str, &str) {
    entry.split_once(':').unwrap_or((entry, ""))
}

/// The composition text for every program in `deployment`. Checks are not services: they are
/// scheduled work, which a container host schedules itself.
#[must_use]
pub fn compose(deployment: &Deployment) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Generated from the deployment manifest ({}). Do not edit.\n",
        deployment.platform_version
    ));
    out.push_str("name: bijective\nservices:\n");

    let mut programs: Vec<&Program> = deployment.programs.iter().collect();
    programs.sort_by(|a, b| a.name.cmp(&b.name));
    let mut volumes = BTreeSet::new();
    let mut secrets = BTreeMap::new();
    for program in programs {
        out.push_str(&service(program, &deployment.platform_version));
        out.push('\n');
        volumes.extend(program.writes.iter().map(|path| volume_name(path)));
        for entry in &program.credentials {
            let (name, path) = credential(entry);
            secrets.insert(name.to_string(), path.to_string());
        }
    }

    out.push_str("volumes:\n");
    for name in &volumes {
        out.push_str(&format!("  {name}: {{}}\n"));
    }
    if !secrets.is_empty() {
        out.push_str("\nsecrets:\n");
        for (name, path) in &secrets {
            out.push_str(&format!("  {name}:\n    file: {}\n", quoted(path)));
        }
    }
    out
}

fn service(program: &Program, platform_version: &str) -> String {
    let mut out = format!("  {}:\n", program.name);
    out.push_str(&format!("    image: ${{BIJECTIVE_IMAGE:-bijective:{platform_version}}}\n"));
    out.push_str(&format!("    entrypoint: [{}]\n", quoted(&program.binary)));
    let arguments: Vec<String> = program.arguments.iter().map(|a| quoted(a)).collect();
    out.push_str(&format!("    command: [{}]\n", arguments.join(", ")));

    if !program.after.is_empty() {
        let mut after: Vec<&str> = program.after.iter().map(String::as_str).collect();
        after.sort_unstable();
        out.push_str("    depends_on:\n");
        for name in after {
            out.push_str(&format!("      {name}:\n        condition: service_started\n"));
        }
    }

    // Restart is never evidence of readiness. The control engine comes back halted.
    out.push_str("    restart: on-failure\n");
    out.push_str(&format!("    stop_grace_period: {}s\n", program.stop_timeout_seconds));
    out.push_str(&format!("    mem_limit: {}m\n", program.memory_max_mb));
    out.push_str("    read_only: true\n    tmpfs:\n      - /tmp\n");
    out.push_str("    cap_drop:\n      - ALL\n    security_opt:\n      - no-new-privileges:true\n");
    out.push_str(&mounts(program));
    if program.class == ProgramClass::Operational {
        out.push_str("    profiles: [\"operational\"]\n");
    }
    out
}

fn mounts(program: &Program) -> String {
    let mut out = String::new();
    let mut writes: Vec<&str> = program.writes.iter().map(String::as_str).collect();
    writes.sort_unstable();
    writes.dedup();
    if !writes.is_empty() {
        out.push_str("    volumes:\n");
        for path in writes {
            out.push_str(&format!("      - {}:{path}\n", volume_name(path)));
        }
    }
    let mut names: Vec<&str> = program.credentials.iter().map(|c| credential(c).0).collect();
    names.sort_unstable();
    if !names.is_empty() {
        out.push_str("    secrets:\n");
        for name in names {
            out.push_str(&format!("      - {name}\n"));
        }
        out.push_str(&format!(
            "    environment:\n      CREDENTIALS_DIRECTORY: {CREDENTIALS_DIRECTORY}\n"
        ));
    }
    out
}
