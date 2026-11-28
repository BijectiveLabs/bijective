// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a launcher beside the supervisor needs: the cgroup subtree a run is bounded under, its
//! sibling programs, and the Python library a strategy imports. A subtree is never guessed: a run
//! that is not bounded is not the run an operator gets.

use std::path::PathBuf;

/// Where the platform's installation page says how to get a Linux host on each computer.
const INSTALLATION: &str = "https://docs.bijectivelabs.dev/bijective/getting_started/installation";

/// Why this operating system cannot bound a run and what to do instead, or `None` on Linux.
/// Asked first, so a first command on Windows or macOS names the fix instead of failing later on a
/// symptom of the same cause (no `python3`, no `systemd-run`).
#[must_use]
pub fn unsupported_host() -> Option<String> {
    host_fix(std::env::consts::OS)
}

/// The refusal for `os`, as `std::env::consts::OS` names it. `None` only for Linux.
#[must_use]
pub fn host_fix(os: &str) -> Option<String> {
    let (host, instead) = match os {
        "linux" => return None,
        "windows" => (
            "Windows",
            "Run it inside WSL 2, the Linux that ships with Windows: `wsl --install -d Ubuntu`, \
             then follow the quick start inside Ubuntu.",
        ),
        "macos" => (
            "macOS",
            "Run it inside a Linux virtual machine with systemd (Ubuntu 24.04 or newer), then \
             follow the quick start inside it.",
        ),
        _ => ("this operating system", "Run it on Linux."),
    };
    Some(format!(
        "this is {host}. A strategy run is bounded with Linux cgroup v2 and namespaces, which \
         {host} does not have, and a run that is not bounded is not the run a client gets. \
         {instead} See {INSTALLATION}"
    ))
}

/// The subtree runs are bounded under: the one named by `BIJECTIVE_CGROUP_ROOT`, or a scope of
/// this session's own. An existing subtree is never adopted unnamed.
pub fn bounded() -> Result<Bounded, String> {
    if let Some(named) = std::env::var_os("BIJECTIVE_CGROUP_ROOT") {
        return Ok(Bounded::Under(named));
    }
    match in_a_scope_of_our_own()? {
        Made::Here(root) => {
            println!("bounded:  {} (a scope of this session's own)", root.display());
            Ok(Bounded::Under(root.into_os_string()))
        }
        Made::Elsewhere(code) => Ok(Bounded::RanInScope(code)),
    }
}

/// What an operator types when no scope could be made.
pub fn say_how_to_bound(program: &str) {
    eprintln!("           A run that is not bounded is not the run a Client gets. Start one:");
    eprintln!();
    eprintln!("    systemd-run --user --scope -p Delegate=yes \\");
    eprintln!("      --setenv=BIJECTIVE_CGROUP_ROOT=<delegated-subtree> -- {program} ...");
}

/// A sibling of this program, so a build tree and an installed node both work.
#[must_use]
pub fn beside_me(name: &str) -> Option<PathBuf> {
    let here = std::env::current_exe().ok()?;
    let beside = here.parent()?.join(name);
    beside.exists().then_some(beside)
}

/// Where `bijective_strategy` can be imported from, if this build carries it. Looked for beside
/// the binary like the supervisor. A missing library otherwise fails inside the sandbox with a
/// traceback that is not shown.
#[must_use]
pub fn library_for_python() -> Option<PathBuf> {
    let here = std::env::current_exe().ok()?;
    let base = here.parent()?;
    for candidate in ["../lib/python", "../../python/bijective-strategy/src"] {
        let root = base.join(candidate);
        if root.join("bijective_strategy").join("__init__.py").exists() {
            return root.canonicalize().ok();
        }
    }
    None
}

/// Where a run is bounded: under a subtree, or nowhere here because this program was run again
/// inside a new scope, which ended with the code given.
pub enum Bounded {
    Under(std::ffi::OsString),
    RanInScope(std::process::ExitCode),
}

enum Made {
    Here(PathBuf),
    Elsewhere(std::process::ExitCode),
}

/// Marks the run that is already inside the scope, so this cannot recurse.
const INSIDE: &str = "BIJECTIVE_SCOPE";

/// Ask the init system for a scope of our own, or set one up if already inside it.
/// Never a subtree delegated to another process: this makes a new one, gone when the session is.
fn in_a_scope_of_our_own() -> Result<Made, String> {
    if std::env::var_os(INSIDE).is_some() {
        return delegate_here().map(Made::Here);
    }

    let Some(runner) = beside_on_path("systemd-run") else {
        return Err("no systemd-run on PATH, and BIJECTIVE_CGROUP_ROOT names nothing".to_string());
    };
    let me = std::env::current_exe().map_err(|e| format!("{e}"))?;

    let status = std::process::Command::new(runner)
        .args([
            "--user",
            "--scope",
            "--quiet",
            "-p",
            "Delegate=yes",
            &format!("--setenv={INSIDE}=1"),
        ])
        .arg("--")
        .arg(me)
        .args(std::env::args_os().skip(1))
        .status()
        .map_err(|e| format!("a scope could not be started: {e}"))?;

    Ok(Made::Elsewhere(if status.success() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }))
}

/// Turn the scope this process is in into a tree it can bound runs under.
/// A cgroup cannot both hold processes and enable controllers for children, so this moves into a leaf first.
fn delegate_here() -> Result<PathBuf, String> {
    let own = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|e| format!("this process's cgroup could not be read: {e}"))?;
    let relative = own
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or("this host does not use cgroup v2")?
        .trim();
    let scope = PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));

    let leader = scope.join("leader");
    std::fs::create_dir_all(&leader)
        .map_err(|e| format!("{} could not be made: {e}", leader.display()))?;
    std::fs::write(leader.join("cgroup.procs"), std::process::id().to_string())
        .map_err(|e| format!("this process could not be moved into {}: {e}", leader.display()))?;

    std::fs::write(scope.join("cgroup.subtree_control"), "+memory +pids")
        .map_err(|e| format!("the scope delegates no controllers: {e}"))?;

    let runs = scope.join("runs");
    std::fs::create_dir_all(&runs)
        .map_err(|e| format!("{} could not be made: {e}", runs.display()))?;
    std::fs::write(runs.join("cgroup.subtree_control"), "+memory +pids")
        .map_err(|e| format!("{} delegates no controllers: {e}", runs.display()))?;

    Ok(runs)
}

fn beside_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|c| c.is_file())
}

#[cfg(test)]
#[path = "../tests/unit/launch_tests.rs"]
mod tests;
