// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Manifest to unit text, deterministically.
//! No timestamp, no host name, no map iteration: two runs over one manifest give the same bytes.

use std::collections::BTreeMap;

use crate::manifest::{Deployment, Program, ProgramClass, ScheduledCheck};

/// The units a manifest produces, keyed by file name so the set is ordered and comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub units: BTreeMap<String, String>,
    /// Unit names the installer enables. Everything else is installed and left stopped.
    pub enable: Vec<String>,
}

impl Generated {
    pub fn concatenated(&self) -> String {
        let mut out = String::new();
        for (name, body) in &self.units {
            out.push_str("# ");
            out.push_str(name);
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
        out
    }
}

pub fn generate(deployment: &Deployment) -> Generated {
    let mut units = BTreeMap::new();
    let mut enable = Vec::new();

    // Output is keyed and sorted, so manifest order does not affect the bytes.
    for program in &deployment.programs {
        let file = format!("bijective-{}.service", program.name);
        units.insert(file.clone(), unit_text(program, &deployment.platform_version));
        if program.class.is_enabled_at_install() {
            enable.push(file);
        }
    }
    // One template per check and one enabled timer per instance, so no venue name enters the unit set.
    for check in &deployment.checks {
        let service = format!("bijective-{}@.service", check.name);
        let timer = format!("bijective-{}@.timer", check.name);
        units.insert(service, check_service(check, &deployment.platform_version));
        units.insert(timer, check_timer(check, &deployment.platform_version));
        for instance in &check.instances {
            enable.push(format!("bijective-{}@{instance}.timer", check.name));
        }
    }
    enable.sort();

    Generated { units, enable }
}

const INSTANCE: &str = "{instance}";

fn check_service(check: &ScheduledCheck, platform_version: &str) -> String {
    let mut out = String::new();
    out.push_str("[Unit]\n");
    out.push_str(&format!("Description=Bijective {} for %i ({platform_version})\n", check.name));
    out.push_str("X-Bijective-Generated=true\n");

    out.push_str("\n[Service]\n");
    // Oneshot: a restarting check would hide a failure behind a retry.
    out.push_str("Type=oneshot\n");
    let mut command = check.binary.clone();
    for argument in &check.arguments {
        command.push(' ');
        command.push_str(&argument.replace(INSTANCE, "%i"));
    }
    out.push_str(&format!("ExecStart={command}\n"));
    out.push_str(&format!("TimeoutStopSec={}\n", check.stop_timeout_seconds));
    out.push_str("NoNewPrivileges=true\n");
    out.push_str("PrivateTmp=true\n");
    out.push_str("ProtectSystem=strict\n");
    out.push_str(&writable(&check.writes));
    out.push_str("ProtectHome=true\n");
    out
}

fn check_timer(check: &ScheduledCheck, platform_version: &str) -> String {
    let mut out = String::new();
    out.push_str("[Unit]\n");
    out.push_str(&format!(
        "Description=Bijective {} schedule for %i ({platform_version})\n",
        check.name
    ));
    out.push_str("X-Bijective-Generated=true\n");

    out.push_str("\n[Timer]\n");
    out.push_str(&format!("OnCalendar={}\n", check.on_calendar));
    // Persistent, so a node that was off through the window checks before it is trusted.
    out.push_str("Persistent=true\n");
    out.push_str(&format!("Unit=bijective-{}@%i.service\n", check.name));

    out.push_str("\n[Install]\n");
    out.push_str("WantedBy=timers.target\n");
    out
}

/// Deny by default: `ProtectSystem=strict` is read-only, so an undeclared path fails on the deployed host.
fn writable(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&str> = paths.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    format!("ReadWritePaths={}\n", sorted.join(" "))
}

fn unit_text(program: &Program, platform_version: &str) -> String {
    let mut after: Vec<String> =
        program.after.iter().map(|n| format!("bijective-{n}.service")).collect();
    after.sort();

    let mut out = String::new();
    out.push_str("[Unit]\n");
    out.push_str(&format!("Description=Bijective {} ({platform_version})\n", program.name));
    out.push_str("X-Bijective-Generated=true\n");
    if !after.is_empty() {
        out.push_str(&format!("After={}\n", after.join(" ")));
        // Requires, not Wants: ordering without need starts this unit after a failed dependency.
        out.push_str(&format!("Requires={}\n", after.join(" ")));
    }

    out.push_str("\n[Service]\n");
    out.push_str("Type=simple\n");
    let mut command = program.binary.clone();
    for argument in &program.arguments {
        command.push(' ');
        command.push_str(argument);
    }
    out.push_str(&format!("ExecStart={command}\n"));

    // Restart is never evidence of readiness. The Control Engine returns halted.
    out.push_str("Restart=on-failure\n");
    out.push_str("RestartSec=5\n");
    out.push_str(&format!("TimeoutStopSec={}\n", program.stop_timeout_seconds));
    out.push_str(&format!("MemoryMax={}M\n", program.memory_max_mb));
    if program.delegates_cgroup {
        // Narrow on purpose: a delegated subtree is authority, needed only to bound agent runs.
        out.push_str("Delegate=yes\n");
    }
    out.push_str("NoNewPrivileges=true\n");
    out.push_str("PrivateTmp=true\n");
    out.push_str("ProtectSystem=strict\n");
    out.push_str("ProtectHome=true\n");
    out.push_str(&writable(&program.writes));
    // Sorted for deterministic output.
    let mut credentials: Vec<&str> = program.credentials.iter().map(String::as_str).collect();
    credentials.sort_unstable();
    for credential in credentials {
        out.push_str(&format!("LoadCredential={credential}\n"));
    }

    out.push_str("\n[Install]\n");
    out.push_str("WantedBy=multi-user.target\n");

    let _ = ProgramClass::Core;
    out
}
