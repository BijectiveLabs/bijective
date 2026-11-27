// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The sandbox's gates: a run that tries and is refused, not a run that does not try.

use std::net::{TcpListener, TcpStream};

use super::*;

/// The child's probe from inside the sandbox. A test, so the program under test and the one run are the same build.
#[test]
fn reach_the_listener() {
    let Ok(port) = std::env::var("BIJECTIVE_SANDBOX_PROBE") else {
        return;
    };
    let target = format!("127.0.0.1:{port}");
    let outcome = TcpStream::connect(&target);
    println!("PROBE reachable={}", outcome.is_ok());
}

/// Named by the operator, never guessed: cgroups made in a temp directory bound nothing, so without the variable tests return rather than assert (`docs/concepts/strategies.md`).
fn delegated() -> Option<PathBuf> {
    std::env::var_os("BIJECTIVE_CGROUP_ROOT").map(PathBuf::from)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-sandbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn limits() -> Limits {
    Limits { memory_bytes: 256 * 1024 * 1024, processes: 64, wall: Duration::from_secs(30) }
}

fn probe_layout(inputs: &Path, port: u16) -> RunLayout {
    let mut environment = BTreeMap::new();
    environment.insert("BIJECTIVE_SANDBOX_PROBE".to_string(), port.to_string());
    environment.insert("PATH".to_string(), "/usr/bin:/bin".to_string());

    RunLayout {
        inputs: inputs.to_path_buf(),
        program: std::env::current_exe().expect("the test binary"),
        args: vec!["--exact".into(), "tests::reach_the_listener".into(), "--nocapture".into()],
        environment,
    }
}

fn unshare_available() -> bool {
    Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "--", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Loopback and a scratch-rooted cgroup make the gate stronger, not weaker (`docs/concepts/strategies.md`).
#[test]
fn a_run_that_tries_to_reach_the_network_is_refused() {
    if !unshare_available() {
        eprintln!("SKIPPED: unprivileged user namespaces unavailable on this host");
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();

    // Vacuity check: the target must be reachable from here, or the refusal below proves nothing.
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "the listener must be reachable from the host for this test to mean anything"
    );

    let dir = scratch("no-network");
    let Some(root) = delegated() else { return };
    let cgroup = RunCgroup::create(&root, "probe", limits()).expect("cgroup");
    let completed = run(&cgroup, &probe_layout(&dir, port), limits(), b"").expect("run");

    let stdout = String::from_utf8_lossy(&completed.stdout);
    assert!(
        stdout.contains("PROBE reachable=false"),
        "the run reached a socket it should not have; output was: {stdout}"
    );
}

/// Without this the refusal test passes whenever the child fails for any reason at all.
#[test]
fn the_same_run_outside_the_sandbox_reaches_it() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();

    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", "tests::reach_the_listener", "--nocapture"])
        .env_clear()
        .env("BIJECTIVE_SANDBOX_PROBE", port.to_string())
        .output()
        .expect("run the probe directly");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PROBE reachable=true"),
        "the probe must succeed unsandboxed, or the sandboxed refusal means nothing: {stdout}"
    );
}

#[test]
fn a_run_that_hangs_is_ended_by_the_supervisor() {
    if !unshare_available() {
        eprintln!("SKIPPED: unprivileged user namespaces unavailable on this host");
        return;
    }

    let dir = scratch("hang");
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), "/usr/bin:/bin".to_string());

    let bound = Limits { wall: Duration::from_millis(300), ..limits() };
    let Some(root) = delegated() else { return };
    let cgroup = RunCgroup::create(&root, "hang", bound).expect("cgroup");
    let completed = run(
        &cgroup,
        &RunLayout {
            inputs: dir.clone(),
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "sleep 30".into()],
            environment,
        },
        bound,
        b"",
    )
    .expect("run");

    assert_eq!(completed.exit, Exit::Timeout);
    assert!(
        completed.elapsed < Duration::from_secs(5),
        "a run consuming no CPU still has to be stopped"
    );
}

#[test]
fn the_environment_is_constructed_rather_than_inherited() {
    if !unshare_available() {
        eprintln!("SKIPPED: unprivileged user namespaces unavailable on this host");
        return;
    }

    // Read, not set: `set_var` is unsafe since the 2024 edition and this crate forbids unsafe.
    let (carried, _) = std::env::vars()
        .find(|(k, _)| k == "HOME" || k == "PATH")
        .expect("this process has an environment");

    let dir = scratch("environment");
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), "/usr/bin:/bin".to_string());

    let Some(root) = delegated() else { return };
    let cgroup = RunCgroup::create(&root, "env", limits()).expect("cgroup");
    let completed = run(
        &cgroup,
        &RunLayout {
            inputs: dir.clone(),
            program: PathBuf::from("/usr/bin/env"),
            args: vec![],
            environment,
        },
        limits(),
        b"",
    )
    .expect("run");

    let stdout = String::from_utf8_lossy(&completed.stdout);
    let names: Vec<&str> = stdout.lines().filter_map(|l| l.split('=').next()).collect();
    assert!(
        names.iter().all(|n| *n != "HOME"),
        "the run inherited the parent's environment: {stdout}"
    );
    assert!(
        names.contains(&"PATH"),
        "the run should have exactly what it was given, and it was given PATH: {stdout}"
    );
    let _ = carried;
}

#[test]
fn a_host_with_no_writable_cgroup_subtree_is_refused() {
    let outcome = probe(Path::new("/proc/self/no-such-cgroup-root"));

    match outcome {
        Err(Missing::CgroupSubtree { .. }) => {}
        other => panic!("expected a named precondition failure, got {other:?}"),
    }
}

/// `create_dir` and a write to `cgroup.procs` both succeed on any writable directory, so an ordinary directory used to pass for a cgroup.
#[test]
fn an_ordinary_writable_directory_is_not_a_cgroup_subtree() {
    let dir = scratch("not-a-cgroup");

    match probe(&dir) {
        Err(Missing::CgroupSubtree { .. }) => {}
        other => panic!("a directory the kernel never made a cgroup was accepted: {other:?}"),
    }
}

/// The probe leaves the root as found: a leftover scratch made the next probe fail with `File exists`.
#[test]
fn the_probe_leaves_the_root_as_it_found_it() {
    let dir = scratch("probe-residue");
    let _ = probe(&dir);

    let left: Vec<_> = std::fs::read_dir(&dir)
        .expect("the root is readable")
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(left.is_empty(), "the probe left {left:?} behind");
}

#[test]
fn a_run_does_not_adopt_a_cgroup_that_is_already_there() {
    let Some(root) = delegated() else { return };
    let held = RunCgroup::create(&root, "adopt", limits()).expect("cgroup");

    match RunCgroup::create(&root, "adopt", limits()) {
        Err(CgroupError::Create(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        other => panic!("a second run took over the first one's cgroup: {other:?}"),
    }
    drop(held);
}

/// A read of the file the host cannot supply is not read as *not killed*.
#[test]
fn an_unreadable_kill_record_is_not_a_clean_exit() {
    let Some(root) = delegated() else { return };
    let cgroup = RunCgroup::create(&root, "no-events", limits()).expect("cgroup");
    // The evidence is removed: a kernel-made cgroup always has `memory.events`, so the case to test is it being unreadable when asked.
    std::fs::remove_dir(cgroup.path()).expect("the run's own cgroup is removable");

    assert!(
        cgroup.was_killed().is_err(),
        "absent evidence must be an error the caller sees, not a false answer"
    );
}

use crate::cgroup::RunCgroup;
use crate::live::{Live, Unsent};

/// A cgroup, or nothing where the host cannot bound a run. `probe` refuses that at start-up, and these tests must not report a pass they did not earn.
fn bounded(name: &str) -> Option<RunCgroup> {
    RunCgroup::create(&delegated()?, name, limits()).ok()
}

fn layout(inputs: &Path, script: &Path) -> RunLayout {
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    RunLayout {
        inputs: inputs.to_path_buf(),
        program: PathBuf::from("/bin/sh"),
        args: vec![script.display().to_string()],
        environment,
    }
}

/// Bounded wait: a live test cannot assert right after sending, and a failure must not hang.
fn until(mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..400 {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn a_live_strategy_answers_while_it_is_still_running() {
    let dir = scratch("live-echo");
    let Some(cgroup) = bounded("live-echo") else {
        return;
    };

    let script = dir.join("echo.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nwhile IFS= read -r line; do printf 'saw %s\\n' \"$line\"; done\n",
    )
    .expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");

    live.send(b"first\n").expect("send");
    let mut heard = Vec::new();
    let arrived = until(|| {
        heard.extend(live.hear().expect("hear").lines);
        !heard.is_empty()
    });
    assert!(arrived, "nothing arrived while the strategy was still running");
    assert_eq!(heard[0], "saw first");

    live.send(b"second\n").expect("send");
    let second = until(|| {
        heard.extend(live.hear().expect("hear").lines);
        heard.len() >= 2
    });
    assert!(second, "the strategy stopped after one event: {heard:?}");
    assert_eq!(heard[1], "saw second");

    let ended = live.stop(&cgroup).expect("stop");
    assert!(ended.elapsed > Duration::ZERO);
}

#[test]
fn a_strategy_that_stops_talking_is_reported_closed() {
    let dir = scratch("live-exit");
    let Some(cgroup) = bounded("live-exit") else {
        return;
    };

    let script = dir.join("once.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'one\\n'\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");

    let mut lines = Vec::new();
    let closed = until(|| {
        let heard = live.hear().expect("hear");
        lines.extend(heard.lines);
        heard.closed
    });
    assert!(closed, "a strategy that exited was never reported closed");
    assert_eq!(lines, vec!["one".to_string()]);

    let _ = live.stop(&cgroup).expect("stop");
}

/// Quiet is not closed: a reader reporting closed whenever nothing arrived would pass the test above and be useless.
#[test]
fn a_strategy_that_is_merely_quiet_is_not_reported_closed() {
    let dir = scratch("live-quiet");
    let Some(cgroup) = bounded("live-quiet") else {
        return;
    };

    let script = dir.join("quiet.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 5\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");
    std::thread::sleep(Duration::from_millis(200));

    let heard = live.hear().expect("hear");
    assert!(heard.lines.is_empty());
    assert!(!heard.closed, "a running strategy was reported as having finished");

    let _ = live.stop(&cgroup).expect("stop");
}

/// Sleeping between asks measured 2ms a turn, most of a stepwise run's cost.
#[test]
fn waiting_returns_as_soon_as_the_strategy_speaks() {
    let dir = scratch("live-wait");
    let Some(cgroup) = bounded("live-wait") else {
        return;
    };

    let script = dir.join("wait-echo.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nwhile IFS= read -r line; do printf 'saw %s\\n' \"$line\"; done\n",
    )
    .expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");
    live.send(b"first\n").expect("send");

    let started = std::time::Instant::now();
    let heard = live.wait(Duration::from_secs(10)).expect("wait");
    let took = started.elapsed();

    assert_eq!(heard.lines, vec!["saw first".to_string()]);
    assert!(took < Duration::from_secs(5), "waiting outlived the answer by {took:?}");

    let _ = live.stop(&cgroup).expect("stop");
}

/// A wait that returned empty at once would turn the caller's loop into a spin.
#[test]
fn waiting_on_a_quiet_strategy_spends_the_patience() {
    let dir = scratch("live-wait-quiet");
    let Some(cgroup) = bounded("live-wait-quiet") else {
        return;
    };

    let script = dir.join("wait-quiet.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 5\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");

    let started = std::time::Instant::now();
    let heard = live.wait(Duration::from_millis(200)).expect("wait");
    let took = started.elapsed();

    assert!(heard.lines.is_empty());
    assert!(!heard.closed, "a running strategy was reported as having finished");
    assert!(took >= Duration::from_millis(200), "the wait returned after {took:?}");

    let _ = live.stop(&cgroup).expect("stop");
}

/// Closed is reported without spending the patience, or every run that ends would stall a full wall bound.
#[test]
fn waiting_on_a_finished_strategy_reports_closed() {
    let dir = scratch("live-wait-closed");
    let Some(cgroup) = bounded("live-wait-closed") else {
        return;
    };

    let script = dir.join("wait-once.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'one\\n'\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");

    let started = std::time::Instant::now();
    let mut lines = Vec::new();
    let mut closed = false;
    while started.elapsed() < Duration::from_secs(10) {
        let heard = live.wait(Duration::from_secs(2)).expect("wait");
        lines.extend(heard.lines);
        if heard.closed {
            closed = true;
            break;
        }
    }

    assert!(closed, "a strategy that exited was never reported closed to a waiting reader");
    assert_eq!(lines, vec!["one".to_string()]);

    let _ = live.stop(&cgroup).expect("stop");
}

/// Asserted on whether the work is gone, not on how long `stop` took: a timing check passed with the group-ending call removed.
#[test]
fn stopping_a_live_strategy_ends_the_work_and_not_only_the_shell() {
    let dir = scratch("live-stop");
    let Some(cgroup) = bounded("live-stop") else {
        return;
    };

    // Found by command line, not its own pid: `unshare --pid` makes `$$` 1 inside, and `pgrep -f` matches command lines, not file contents.
    let script = dir.join("forever.sh");
    let marker = script.display().to_string();
    std::fs::write(&script, "#!/bin/sh\nwhile :; do sleep 1; done\n").expect("write");

    let running = || {
        std::process::Command::new("pgrep")
            .args(["-f", &marker])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    };

    let live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");
    assert!(
        until(running),
        "the strategy never started, so there is nothing to prove about ending it"
    );

    live.stop(&cgroup).expect("stop");
    assert!(until(|| !running()), "the work outlived the shell that started it");
}

/// A live session has no wall bound: a strategy that stops reading must fill a queue and be noticed, not block the supervisor in a write.
#[test]
fn a_live_strategy_that_stops_reading_is_noticed_rather_than_waited_on() {
    let dir = scratch("live-deaf");
    let Some(cgroup) = bounded("live-deaf") else {
        return;
    };

    let script = dir.join("deaf.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");

    let event = vec![b'x'; 200];
    let started = std::time::Instant::now();
    let mut refused = None;
    for _ in 0..5_000 {
        let mut line = event.clone();
        line.push(b'\n');
        if let Err(e) = live.send(&line) {
            refused = Some(e);
            break;
        }
        if started.elapsed() > Duration::from_secs(20) {
            break;
        }
    }

    assert_eq!(
        refused,
        Some(Unsent::Behind),
        "the supervisor was still handing events to a strategy that reads none, after {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the send waited on the strategy for {:?}",
        started.elapsed()
    );

    let _ = live.stop(&cgroup).expect("stop");
}

/// The bound is the caller's: asserted against the default, not an exact count, since the writer thread may move an event or two into the pipe.
#[test]
fn a_session_holds_what_it_was_told_to_hold() {
    let dir = scratch("live-small-queue");
    let Some(cgroup) = bounded("live-small-queue") else {
        return;
    };

    let script = dir.join("deaf-small.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), 4).expect("start");

    let mut accepted = 0;
    for _ in 0..crate::live::QUEUED_BY_DEFAULT {
        if live.send(b"an event\n").is_err() {
            break;
        }
        accepted += 1;
    }

    assert!(
        accepted < 100,
        "a session told to hold 4 took {accepted}, so the bound it was given was not the one it used"
    );

    let _ = live.stop(&cgroup).expect("stop");
}

/// The other direction: a `send` that reported full whenever busy would pass the test above and end every healthy session.
#[test]
fn a_live_strategy_that_keeps_reading_is_never_left_behind() {
    let dir = scratch("live-keeping-up");
    let Some(cgroup) = bounded("live-keeping-up") else {
        return;
    };

    let script = dir.join("reader.sh");
    std::fs::write(&script, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");

    let started = std::time::Instant::now();
    let mut sent = 0;
    for _ in 0..3_000 {
        loop {
            match live.send(b"an event a strategy is expected to read\n") {
                Ok(()) => {
                    sent += 1;
                    break;
                }
                // Transient: a reading strategy drains the queue. A permanent one is the case above, told apart by the bound.
                Err(Unsent::Behind) if started.elapsed() < Duration::from_secs(20) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("a reading strategy was not sent its events: {e}"),
            }
        }
    }

    assert_eq!(sent, 3_000);

    let _ = live.stop(&cgroup).expect("stop");
}

/// A session ends by closing the strategy's input: ending the group first records every live session as cut short, and its proposals then have no path.
#[test]
fn a_live_strategy_that_reads_until_its_input_closes_ends_cleanly() {
    let dir = scratch("live-goodbye");
    let Some(cgroup) = bounded("live-goodbye") else {
        return;
    };

    let script = dir.join("until-eof.sh");
    std::fs::write(&script, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");
    live.send(b"an event\n").expect("send");

    let ended = live.stop(&cgroup).expect("stop");
    assert_eq!(
        ended.exit,
        Exit::Completed { status: 0 },
        "a strategy reading until end of input was killed rather than left to finish"
    );
}

/// Output past the pipe's 64 KB must still be collected: the old collector read after exit and the run was cut short at the wall bound.
#[test]
fn a_run_that_prints_more_than_a_pipe_holds_is_collected_in_full() {
    let dir = scratch("chatty");
    let Some(cgroup) = bounded("chatty") else {
        return;
    };

    let script = dir.join("chatty.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ni=0\nwhile [ $i -lt 4000 ]; do printf '%s\\n' \
         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; i=$((i+1)); done\ncat > /dev/null\n",
    )
    .expect("write");

    let bound =
        Limits { memory_bytes: 256 * 1024 * 1024, processes: 64, wall: Duration::from_secs(20) };
    let completed = run(&cgroup, &layout(&dir, &script), bound, b"one\ntwo\n").expect("run");

    assert_eq!(
        completed.exit,
        Exit::Completed { status: 0 },
        "a run that printed 164 KB was cut short rather than read"
    );
    assert_eq!(
        completed.stdout.iter().filter(|b| **b == b'\n').count(),
        4000,
        "the run's output arrived short: {} bytes",
        completed.stdout.len()
    );
}

/// The error channel of a live session: a logging strategy blocks on a full pipe and stops reading events.
#[test]
fn a_live_strategy_that_logs_past_a_pipe_still_answers() {
    let dir = scratch("live-noisy");
    let Some(cgroup) = bounded("live-noisy") else {
        return;
    };

    let script = dir.join("noisy.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ni=0\nwhile [ $i -lt 4000 ]; do printf '%s\\n' \
         'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' >&2; i=$((i+1)); done\n\
         while IFS= read -r line; do printf 'saw %s\\n' \"$line\"; done\n",
    )
    .expect("write");

    let mut live = Live::start(&cgroup, &layout(&dir, &script), crate::live::QUEUED_BY_DEFAULT)
        .expect("start");
    live.send(b"first\n").expect("send");

    let heard = live.wait(Duration::from_secs(20)).expect("wait");
    assert_eq!(
        heard.lines,
        vec!["saw first".to_string()],
        "a strategy that logged 164 KB never got to its first event"
    );

    let ended = live.stop(&cgroup).expect("stop");
    assert_eq!(
        ended.stderr.iter().filter(|b| **b == b'\n').count(),
        4000,
        "what it logged was not kept"
    );
}

/// A run cannot time itself, including by not reading its input: the bound has to cover the send as well as the wait.
#[test]
fn a_run_that_never_reads_its_window_is_still_ended_by_the_supervisor() {
    let dir = scratch("deaf");
    let bound =
        Limits { memory_bytes: 256 * 1024 * 1024, processes: 64, wall: Duration::from_millis(500) };
    let Ok(cgroup) = RunCgroup::create(&dir, "deaf", bound) else {
        return;
    };

    let script = dir.join("deaf.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("write");

    // Larger than a pipe buffer, so the write cannot complete without the run reading.
    let window = vec![b'x'; 512 * 1024];

    let started = std::time::Instant::now();
    let completed = run(&cgroup, &layout(&dir, &script), bound, &window).expect("run");

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the supervisor blocked writing to a run that never read, for {:?}",
        started.elapsed()
    );
    assert_eq!(completed.exit, Exit::Timeout);
}
