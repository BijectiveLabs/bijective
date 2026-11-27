// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Bounding an agent run. `docs/concepts/strategies.md` carries why each rule is what it is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub mod cgroup;

pub use cgroup::{CgroupError, RunCgroup};

/// Checked by construction, not by reading a knob: knob names differ between kernels and a missing file proves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    UnshareAbsent,
    UserNamespaces,
    /// No cgroup v2 subtree this process may write to. Separate from the namespace check since the remedies differ.
    CgroupSubtree {
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory_bytes: u64,
    pub processes: u32,
    /// Held by the supervisor: a run cannot time itself, and one that hangs without CPU consumes no limit that would stop it.
    pub wall: Duration,
}

/// `inputs` is the directory it starts in and all of the filesystem it can reach. Events arrive on standard input.
/// A run receives only what the supervisor sends, so its window is a fact about what it has.
#[derive(Debug, Clone)]
pub struct RunLayout {
    pub inputs: PathBuf,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Constructed from nothing: inherit-then-remove is a deny-list that is not extended when a new variable appears.
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Completed { status: i32 },
    LimitExceeded,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

pub mod live;

/// Refuses rather than degrades: a run without isolation yields a verdict that looks like every other, and nothing records the controls were absent.
pub fn probe(cgroup_root: &Path) -> Result<(), Missing> {
    let probe = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "--", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match probe {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Missing::UnshareAbsent),
        Err(e) => {
            return Err(Missing::CgroupSubtree { detail: e.to_string() });
        }
        Ok(status) if !status.success() => return Err(Missing::UserNamespaces),
        Ok(_) => {}
    }

    cgroup::probe(cgroup_root)
}

/// One process per run: arguments are assembled fresh and nothing is reused.
fn isolation_args() -> [&'static str; 6] {
    // An empty network namespace rather than a firewall rule: a rule is configuration an operator can
    // change, and a namespace with no interface has nothing to change.
    ["--user", "--map-root-user", "--net", "--pid", "--mount", "--fork"]
}

/// End every process in the run's group, through `kill(1)` like `unshare(1)`, so the crate stays inside its `unsafe` prohibition.
fn end_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Both lifetimes spawn here so their isolation cannot differ.
/// Own process group: `unshare --fork` puts the work in a grandchild that outlives the direct child (`docs/concepts/strategies.md`).
fn spawn(cgroup: &RunCgroup, layout: &RunLayout) -> Result<std::process::Child, std::io::Error> {
    let mut script = format!(
        "printf %s $$ > '{}' || exit 111; exec unshare {} -- '{}'",
        cgroup.procs_path().display(),
        isolation_args().join(" "),
        layout.program.display()
    );
    for arg in &layout.args {
        script.push_str(&format!(" '{arg}'"));
    }

    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&script)
        .current_dir(&layout.inputs)
        .env_clear()
        .envs(&layout.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    in_own_group(&mut command)?;
    command.spawn()
}

#[cfg(unix)]
fn in_own_group(command: &mut Command) -> Result<(), std::io::Error> {
    std::os::unix::process::CommandExt::process_group(command, 0);
    Ok(())
}

/// Refused rather than run: without a process group the run's grandchild cannot be ended, so the run would not be bounded.
#[cfg(not(unix))]
fn in_own_group(_command: &mut Command) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "a bounded run needs Linux process groups: run the node on Linux (on Windows, inside WSL)",
    ))
}

/// Read as written, on a thread: this crate forbids `unsafe`, and the alternative is non-blocking I/O on a raw descriptor.
pub(crate) fn collect<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
) -> Option<std::thread::JoinHandle<Vec<u8>>> {
    pipe.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut said = Vec::new();
            // A failed read leaves what arrived: half a run's output is more accurate than none.
            let _ = std::io::Read::read_to_end(&mut pipe, &mut said);
            said
        })
    })
}

/// Run `layout` under `limits` in a cgroup of its own, sending `input` as the whole window. Stdin is closed after it (`docs/concepts/strategies.md`).
/// The child writes its own pid into `cgroup.procs` then execs, so the bounded process does the work. The gap is shell start-up with no operator code.
pub fn run(
    cgroup: &RunCgroup,
    layout: &RunLayout,
    limits: Limits,
    input: &[u8],
) -> Result<Completed, std::io::Error> {
    let started = Instant::now();
    let mut child = spawn(cgroup, layout)?;

    // Sent on a thread because the bound has to cover the send: a run that never reads would fill the pipe and block the writer ahead of the deadline loop (`docs/concepts/strategies.md`).
    // A broken pipe is not a fault: the run may have decided everything it will. The pipe is dropped either way so it sees the end of its window.
    let sending = child.stdin.take().map(|mut pipe| {
        let window = input.to_vec();
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = pipe.write_all(&window).and_then(|()| pipe.flush());
        })
    });

    // Read while the run is still writing: reading after exit let a chatty run fill its output pipe and deadlock against the supervisor until the wall bound.
    let saying = collect(child.stdout.take());
    let complaining = collect(child.stderr.take());

    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() >= limits.wall {
                    end_group(child.id());
                    let _ = child.kill();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }

    let status = child.wait()?;
    let elapsed = started.elapsed();

    // Joined after the child is gone so the writer's pipe is closed. Joining first would wait on the send this bound exists to escape.
    if let Some(handle) = sending {
        let _ = handle.join();
    }
    let stdout = saying.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = complaining.and_then(|h| h.join().ok()).unwrap_or_default();

    let exit = if timed_out {
        Exit::Timeout
    } else if cgroup.was_killed().unwrap_or(false) {
        // Read from the cgroup, not inferred from a signal: a memory kill and any other kill exit alike.
        Exit::LimitExceeded
    } else {
        Exit::Completed { status: status.code().unwrap_or(-1) }
    };

    let _ = limits.processes;

    Ok(Completed { exit, stdout, stderr, elapsed })
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
