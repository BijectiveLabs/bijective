// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What bounded runs and live sessions share: the program clock, the sandbox set-up and session marks.

use std::collections::BTreeMap;
use std::path::Path;

use bijective_common::clock::{Clock, WallClock};
use bijective_event_store::digest::Digest;
use bijective_event_store::store::StreamWriter;
use bijective_sandbox::cgroup::RunCgroup;
use bijective_sandbox::{Limits, RunLayout};
use bijective_strategy_engine::session::SessionMark;

/// One wall clock per program, anchored at start. Every time read goes through it.
pub(crate) fn now_ns() -> i128 {
    static CLOCK: std::sync::OnceLock<WallClock> = std::sync::OnceLock::new();
    CLOCK.get_or_init(WallClock::new).now_ns()
}

/// The memory, process and wall-time bound every strategy run and session gets.
pub(crate) fn limits() -> Limits {
    Limits {
        memory_bytes: 256 * 1024 * 1024,
        processes: 64,
        wall: bijective_strategy_engine::conservative_wall(),
    }
}

/// A bounded cgroup and the layout the program starts in.
pub(crate) struct Sandbox {
    pub(crate) cgroup: RunCgroup,
    pub(crate) layout: RunLayout,
}

/// Creates the run directory and a cgroup named `<prefix>-<identity>`, so two runs on one node do
/// not share a cgroup. `noun` ("run" or "session") names the subject in error messages.
pub(crate) fn sandbox(
    cgroup_root: &Path,
    store: &Path,
    program: &Path,
    environment: BTreeMap<String, String>,
    prefix: &str,
    identity: Digest,
    noun: &str,
) -> Result<Sandbox, String> {
    let inputs = store.join("run");
    std::fs::create_dir_all(&inputs)
        .map_err(|e| format!("the {noun} has nowhere to start: {e}"))?;
    let cgroup = RunCgroup::create(cgroup_root, &format!("{prefix}-{}", hex(identity)), limits())
        .map_err(|e| format!("the {noun} could not be bounded: {e:?}"))?;
    let layout =
        RunLayout { inputs, program: program.to_path_buf(), args: Vec::new(), environment };
    Ok(Sandbox { cgroup, layout })
}

/// Records how far a session has been sent through one market stream.
pub(crate) fn mark(
    into: &mut StreamWriter,
    strategy: Digest,
    (stream, sent_through): (&str, u64),
    at_ns: i128,
) -> Result<(), String> {
    let stream = Some(stream.to_string());
    let payload = bijective_strategy_engine::session::encode(&SessionMark {
        strategy,
        market_stream: stream,
        sent_through,
        at_ns,
    })
    .map_err(|e| format!("a session mark could not be encoded: {e:?}"))?;
    into.append(&payload).map_err(|e| format!("a session mark could not be recorded: {e:?}"))?;
    Ok(())
}

/// The first 8 bytes of a run identity as hex, short enough to name a directory.
pub(crate) fn hex(digest: Digest) -> String {
    digest.0.iter().take(8).map(|b| format!("{b:02x}")).collect()
}
