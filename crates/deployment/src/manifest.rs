// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the release says it installs.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: u32 = 1;

/// Core or operational. Operational programs are installed and not enabled, so *stopped* is a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramClass {
    Core,
    Operational,
}

impl ProgramClass {
    pub fn is_enabled_at_install(self) -> bool {
        matches!(self, Self::Core)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    pub binary: String,
    pub class: ProgramClass,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    pub memory_max_mb: u32,
    /// Seconds the init system waits before concluding a stop has hung.
    pub stop_timeout_seconds: u32,
    /// Whether the program needs its own cgroup subtree. Only the Strategy Engine supervisor does.
    /// Off by default: a delegated subtree is authority.
    #[serde(default)]
    pub delegates_cgroup: bool,
    /// Paths the program must write. The unit mounts the filesystem read-only.
    #[serde(default)]
    pub writes: Vec<String>,
    /// Credentials as `name:path`. The manifest names where the secret is, never the secret.
    #[serde(default)]
    pub credentials: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub version: u32,
    pub platform_version: String,
    pub programs: Vec<Program>,
    /// Work the kit schedules rather than keeps running. Empty means nothing is scheduled, by choice.
    #[serde(default)]
    pub checks: Vec<ScheduledCheck>,
}

/// One check run on a calendar, per instance. A template, so venue names stay out of the unit set.
/// `{instance}` in an argument becomes the init system's instance substitution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledCheck {
    pub name: String,
    pub binary: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    /// The calendar expression, passed to the init system verbatim.
    pub on_calendar: String,
    /// What the check is run for. Each becomes an enabled timer instance.
    #[serde(default)]
    pub instances: Vec<String>,
    pub stop_timeout_seconds: u32,
    #[serde(default)]
    pub writes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Unreadable,
    VersionUnknown {
        found: u32,
    },
    NoPrograms,
    DuplicateName {
        name: String,
    },
    /// Refused, not dropped: an unknown `After=` is accepted by the init system and the ordering vanishes.
    DanglingOrder {
        program: String,
        after: String,
    },
    OrderingCycle {
        program: String,
    },
    EmptyField {
        program: String,
        field: &'static str,
    },
    ZeroBound {
        program: String,
        field: &'static str,
    },
    NoCoreProgram,
    /// Refused: a check with no timer looks installed and never runs.
    NoCalendar {
        check: String,
    },
    EmptyCheckField {
        check: String,
        field: &'static str,
    },
    DuplicateCheck {
        name: String,
    },
    /// An instance must be a single path-safe token, or the init system renames it.
    UnusableInstance {
        check: String,
        instance: String,
    },
}

impl core::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unreadable => write!(f, "the deployment manifest could not be read"),
            Self::VersionUnknown { found } => {
                write!(f, "manifest version {found} is not version {MANIFEST_VERSION}")
            }
            Self::NoPrograms => write!(f, "the manifest installs no programs"),
            Self::DuplicateName { name } => write!(f, "{name} is declared twice"),
            Self::DanglingOrder { program, after } => {
                write!(
                    f,
                    "{program} is ordered after {after}, which this manifest does not install"
                )
            }
            Self::OrderingCycle { program } => write!(f, "{program} is ordered after itself"),
            Self::EmptyField { program, field } => write!(f, "{program} has an empty {field}"),
            Self::ZeroBound { program, field } => write!(f, "{program} has a {field} of zero"),
            Self::NoCoreProgram => write!(f, "the manifest declares no core program"),
            Self::NoCalendar { check } => {
                write!(f, "check {check} has no calendar, so nothing would ever run it")
            }
            Self::EmptyCheckField { check, field } => {
                write!(f, "check {check} leaves {field} empty")
            }
            Self::DuplicateCheck { name } => write!(f, "{name} is declared twice"),
            Self::UnusableInstance { check, instance } => write!(
                f,
                "check {check} names an instance the init system could not carry: {instance:?}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Deployment {
    pub fn parse(raw: &[u8]) -> Result<Self, ManifestError> {
        let parsed: Self = serde_json::from_slice(raw).map_err(|_| ManifestError::Unreadable)?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Every refusal here is one an init system would otherwise accept quietly.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version != MANIFEST_VERSION {
            return Err(ManifestError::VersionUnknown { found: self.version });
        }
        if self.programs.is_empty() {
            return Err(ManifestError::NoPrograms);
        }
        let mut seen = BTreeSet::new();
        for p in &self.programs {
            validate_program(p, &mut seen)?;
        }
        for p in &self.programs {
            for after in &p.after {
                if after == &p.name {
                    return Err(ManifestError::OrderingCycle { program: p.name.clone() });
                }
                if !seen.contains(after) {
                    return Err(ManifestError::DanglingOrder {
                        program: p.name.clone(),
                        after: after.clone(),
                    });
                }
            }
        }

        self.detect_cycle()?;

        if !self.programs.iter().any(|p| p.class == ProgramClass::Core) {
            return Err(ManifestError::NoCoreProgram);
        }
        // Checks share the name space with programs: one unit name per installed thing.
        for check in &self.checks {
            validate_check(check, &mut seen)?;
        }
        Ok(())
    }

    fn detect_cycle(&self) -> Result<(), ManifestError> {
        for start in &self.programs {
            let mut seen = BTreeSet::new();
            let mut stack = vec![start.name.clone()];
            while let Some(current) = stack.pop() {
                if !seen.insert(current.clone()) {
                    continue;
                }
                let Some(p) = self.programs.iter().find(|p| p.name == current) else {
                    continue;
                };
                for after in &p.after {
                    if after == &start.name {
                        return Err(ManifestError::OrderingCycle { program: start.name.clone() });
                    }
                    stack.push(after.clone());
                }
            }
        }
        Ok(())
    }

    /// Units the installer enables. Operational programs are installed and left stopped.
    pub fn enabled_at_install(&self) -> Vec<&Program> {
        self.programs.iter().filter(|p| p.class.is_enabled_at_install()).collect()
    }
}

/// Non-empty name and binary, non-zero bounds, and a name no other program uses.
fn validate_program(p: &Program, seen: &mut BTreeSet<String>) -> Result<(), ManifestError> {
    if p.name.trim().is_empty() {
        return Err(ManifestError::EmptyField { program: p.binary.clone(), field: "name" });
    }
    if p.binary.trim().is_empty() {
        return Err(ManifestError::EmptyField { program: p.name.clone(), field: "binary" });
    }
    if p.memory_max_mb == 0 {
        return Err(ManifestError::ZeroBound { program: p.name.clone(), field: "memory_max_mb" });
    }
    if p.stop_timeout_seconds == 0 {
        return Err(ManifestError::ZeroBound {
            program: p.name.clone(),
            field: "stop_timeout_seconds",
        });
    }
    if !seen.insert(p.name.clone()) {
        return Err(ManifestError::DuplicateName { name: p.name.clone() });
    }
    Ok(())
}

/// Non-empty name, binary and calendar, a non-zero stop bound, a unique name, and instance names
/// usable in a unit name.
fn validate_check(
    check: &ScheduledCheck,
    seen: &mut BTreeSet<String>,
) -> Result<(), ManifestError> {
    if check.name.trim().is_empty() {
        return Err(ManifestError::EmptyCheckField { check: check.binary.clone(), field: "name" });
    }
    if check.binary.trim().is_empty() {
        return Err(ManifestError::EmptyCheckField { check: check.name.clone(), field: "binary" });
    }
    if check.on_calendar.trim().is_empty() {
        return Err(ManifestError::NoCalendar { check: check.name.clone() });
    }
    if check.stop_timeout_seconds == 0 {
        return Err(ManifestError::ZeroBound {
            program: check.name.clone(),
            field: "stop_timeout_seconds",
        });
    }
    if !seen.insert(check.name.clone()) {
        return Err(ManifestError::DuplicateCheck { name: check.name.clone() });
    }
    for instance in &check.instances {
        if instance.trim().is_empty()
            || !instance.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ManifestError::UnusableInstance {
                check: check.name.clone(),
                instance: instance.clone(),
            });
        }
    }
    Ok(())
}
