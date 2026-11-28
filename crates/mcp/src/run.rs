// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Running one platform program for a tool: located beside this server, bounded in time, and
//! answered with what it printed.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// More than this many bytes of output are cut, and the cut is said. An agent's context is finite
/// and a runaway program must not fill it.
pub(crate) const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

const POLL: Duration = Duration::from_millis(50);

/// What a program did.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// `None` when it was ended for running past its bound.
    pub(crate) exit_code: Option<i32>,
    pub(crate) output: String,
}

impl Outcome {
    pub(crate) fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// The program beside this server first, then on `PATH`. Beside first, so a release runs the
/// programs it was shipped with.
pub(crate) fn locate(name: &str) -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join(name)));
    if let Some(path) = beside.filter(|p| p.is_file()) {
        return Some(path);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Runs `program` in `cwd` and waits at most `bound`. A program past its bound is killed and the
/// output it had written is kept. The program runs in a process group of its own and the whole
/// group is ended, because a program's children hold its output open after it is gone.
///
/// # Errors
/// The program could not be started.
pub(crate) fn run(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    bound: Duration,
) -> Result<Outcome, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", program.display()))?;
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);

    let started = Instant::now();
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() >= bound => {
                end_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(format!("{}: {e}", program.display())),
        }
    };

    let mut output = collect(out);
    let stderr = collect(err);
    if !stderr.is_empty() {
        output.push_str(&stderr);
    }
    if exit_code.is_none() {
        output.push_str(&format!("\n[ended after {} s, its bound]\n", bound.as_secs()));
    }
    Ok(Outcome { exit_code, output: truncated(output) })
}

/// Through `kill(1)`, as the sandbox does, so this crate stays inside its `unsafe` prohibition.
fn end_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Read on a thread: a pipe nobody reads fills and stops the child.
fn drain<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

fn collect(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> String {
    let bytes = handle.and_then(|h| h.join().ok()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn truncated(mut text: String) -> String {
    if text.len() <= OUTPUT_LIMIT_BYTES {
        return text;
    }
    let mut cut = OUTPUT_LIMIT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = text.len() - cut;
    text.truncate(cut);
    text.push_str(&format!("\n[{dropped} more bytes not shown]\n"));
    text
}

#[cfg(test)]
#[path = "../tests/unit/run_tests.rs"]
mod tests;
