// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Per-run limits written to a cgroup created for the run and removed when it ends.
//! Per run, not shared: a budget the noisiest run spends for the others would let one run exhaust it for the next.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Limits, Missing};

#[derive(Debug)]
pub enum CgroupError {
    Create(std::io::Error),
    Write { file: &'static str, source: std::io::Error },
}

impl std::fmt::Display for CgroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CgroupError::Create(e) => write!(f, "could not create the run's cgroup: {e}"),
            CgroupError::Write { file, source } => write!(f, "could not write {file}: {source}"),
        }
    }
}

impl std::error::Error for CgroupError {}

/// Attempted, not inspected: permission on a delegated subtree depends on ownership, mount and delegation, none readable from one file.
pub fn probe(root: &Path) -> Result<(), Missing> {
    let scratch = root.join("bijective-probe");
    let outcome = fs::create_dir(&scratch)
        .and_then(|()| kernel_made_it(&scratch))
        .and_then(|()| fs::write(scratch.join("cgroup.procs"), b""))
        .map_err(|e| e.to_string());
    // Empty wherever removal could fail, since kernel_made_it gates the file write. A leftover scratch made the next probe refuse a healthy host.
    let _ = fs::remove_dir(&scratch);

    match outcome {
        Ok(()) => Ok(()),
        Err(detail) => Err(Missing::CgroupSubtree { detail }),
    }
}

/// Whether the kernel made this directory a cgroup, or the filesystem merely made it a directory.
/// Without this check any writable directory passed for a cgroup and the limits went to ordinary files.
fn kernel_made_it(dir: &Path) -> std::io::Result<()> {
    if dir.join("cgroup.procs").exists() {
        return Ok(());
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "{} is a directory the kernel put nothing in, so it is not a cgroup",
            dir.display()
        ),
    ))
}

/// One run's cgroup. Removed when the run ends, which is what makes the limits per run.
#[derive(Debug)]
pub struct RunCgroup {
    dir: PathBuf,
}

impl RunCgroup {
    pub fn create(root: &Path, run_id: &str, limits: Limits) -> Result<Self, CgroupError> {
        let dir = root.join(format!("bijective-run-{run_id}"));
        // `create_dir`, so an existing one is refused: adopting a killed supervisor's cgroup would mean someone else's limits and `memory.events`.
        fs::create_dir(&dir).map_err(CgroupError::Create)?;
        // Held here as well as in `probe`: a `RunCgroup` over a directory the kernel never made would carry the bounded claim for every run.
        kernel_made_it(&dir).map_err(CgroupError::Create)?;

        let held = Self { dir };
        held.write("memory.max", &limits.memory_bytes.to_string())?;
        held.write("pids.max", &limits.processes.to_string())?;
        // Swap disabled: a memory bound a run can page around is not a bound.
        let _ = held.try_write("memory.swap.max", "0");
        Ok(held)
    }

    fn write(&self, file: &'static str, value: &str) -> Result<(), CgroupError> {
        fs::write(self.dir.join(file), value).map_err(|source| CgroupError::Write { file, source })
    }

    fn try_write(&self, file: &str, value: &str) -> std::io::Result<()> {
        fs::write(self.dir.join(file), value)
    }

    #[must_use]
    pub fn procs_path(&self) -> PathBuf {
        self.dir.join("cgroup.procs")
    }

    /// Whether the kernel terminated anything here for exceeding memory. An exit code cannot tell a kernel kill from the run's own failure.
    pub fn was_killed(&self) -> std::io::Result<bool> {
        let events = fs::read_to_string(self.dir.join("memory.events"))?;
        Ok(events
            .lines()
            .filter_map(|l| l.strip_prefix("oom_kill "))
            .any(|count| count.trim() != "0"))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for RunCgroup {
    /// Removal must happen even on an unwind: a cgroup left behind accumulates until an operator notices.
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.dir);
    }
}
