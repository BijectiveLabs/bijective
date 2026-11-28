# 0022: A Linux host on every computer

| Field | Value |
|---|---|
| Status | Accepted and built, 2026-11-28 (`crates/strategy_engine/src/launch.rs`, [installation](../getting_started/installation.md#on-your-computer)) |
| Extends | The sandbox ([strategies](../concepts/strategies.md)) and the first run ([quickstart](../getting_started/quickstart.md)) |

## Context

A strategy run is bounded by the Linux kernel: a cgroup v2 of its own for memory and processes, an
empty network namespace, and no credentials. Windows and macOS have neither. Someone trying the
platform on their own computer before a server is the person this platform most needs to keep,
and on Windows the first command failed on a symptom (measured 2026-11-28: `no python3 on PATH`)
without saying that the cause was the operating system.

Container and cluster tools meet the same constraint, since they are built on the same kernel
features, and answer it the same way: on Windows and macOS they run inside a Linux (WSL 2 or a
virtual machine) rather than reimplementing isolation for each operating system.

## Decision

1. **The bound is not relaxed for any computer.** A run without it is not the run a client gets,
   and a verdict from it would look like every other. There is no unbounded mode.
2. **On Windows and macOS the platform runs inside a Linux.** On Windows that is WSL 2, one
   command (`wsl --install -d Ubuntu`). On macOS it is an Ubuntu virtual machine, started with
   Lima (`brew install lima`, `limactl start`). Inside it, nothing about the platform differs from a server, so a first run
   there moves to a VPS unchanged.
3. **The first command says so.** `bijective-backtest` and `bijective-paper` refuse to start on a
   computer that is not Linux, before any other check, name the computer, give its one fix, and
   point to the installation page.
4. **The release artefact is the first-run path.** Inside the Linux, the published archive is
   downloaded, checked and unpacked, with no Rust toolchain and no build.

## Shorter path, and what a longer one would add

This is the shortest: one check at the start of two programs, one page, and an artefact for each
architecture, so a Mac with Apple silicon runs the release without a build. A longer path would add a
container image that carries Python and runs the sandbox inside it. It is added when it is
measured, not before.
