#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Proves a run over its memory bound is terminated and the bound is per run. Not a change gate, as build machines have no delegated subtree.
# Must already run inside a cgroup v2 subtree delegated to this user (a login scope belongs to root): systemd-run --user --scope --setenv=BIJECTIVE_CGROUP_ROOT=<subtree> -- ./scripts/verify/sandbox.sh
# Exit codes: 0 every check that could run passed, 1 otherwise.

set -uo pipefail

say() { printf '%s\n' "$*"; }

ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the limits are unproven until it runs"
    exit 1
fi

if [ ! -d "$ROOT" ]; then
    say "FAIL  ${ROOT} is not a directory"
    exit 1
fi

# A cgroup v2 node cannot both hold processes and enable controllers for its children. A drill started
# inside the subtree moves the subtree's processes into a leaf first, as the supervisor does at start.
if grep -qx "$$" "${ROOT}/cgroup.procs" 2>/dev/null; then
    mkdir -p "${ROOT}/leader" || { say "FAIL  cannot create a leaf under ${ROOT}"; exit 1; }
    while read -r pid; do
        printf '%s' "$pid" > "${ROOT}/leader/cgroup.procs" 2>/dev/null || true
    done < "${ROOT}/cgroup.procs"
    if ! printf '%s' "+memory +pids" > "${ROOT}/cgroup.subtree_control" 2>/dev/null; then
        say "FAIL  the subtree does not delegate memory; ask the operator to enable the controller"
        exit 1
    fi
fi

CG="${ROOT}/bijective-drill-$$"
mkdir "$CG" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${ROOT}"; exit 1; }
trap 'rmdir "$CG" 2>/dev/null || true' EXIT

if ! printf '%s' "$((64 * 1024 * 1024))" > "${CG}/memory.max" 2>/dev/null; then
    say "FAIL  the subtree does not delegate memory; ask the operator to enable the controller"
    exit 1
fi
printf '%s' 0 > "${CG}/memory.swap.max" 2>/dev/null || true
printf '%s' 64 > "${CG}/pids.max" 2>/dev/null || true

# A run that really tries to exceed its bound: command substitution buffers the stream, so the allocation is real.
# The source is yes, not /dev/zero: piping that through tr -d "\0" deleted every byte, so the run never neared its bound.
# stderr is closed on the run so the drill's verdict shows, not the shell's note of a killed job.
( /bin/sh -c 'printf %s $$ > "'"$CG"'/cgroup.procs" || exit 111
              out=$(yes | head -c 200000000)
              printf %s "${#out}"' ) >/dev/null 2>&1
outcome=$?

killed=0
if [ -r "${CG}/memory.events" ]; then
    kills="$(awk '/^oom_kill /{print $2}' "${CG}/memory.events")"
    [ -n "$kills" ] && [ "$kills" != "0" ] && killed=1
fi

if [ "$killed" = 1 ]; then
    say "PASS  a run exceeding its memory bound was terminated (oom_kill recorded, exit ${outcome})"
else
    say "FAIL  the run was not terminated; the bound was not in force (exit ${outcome})"
    exit 1
fi

# Per run means the cgroup goes when the run does, or empty limit directories accumulate.
rmdir "$CG" 2>/dev/null || { say "FAIL  the run's cgroup could not be removed"; exit 1; }
trap - EXIT
say "PASS  the run's cgroup was removed with the run"
say "PASS  limits are per run"
exit 0
