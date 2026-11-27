#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Measures what a live session's queue is worth in seconds at caller-stated arrival rates.
# Usage: systemd-run --user --scope --setenv=BIJECTIVE_CGROUP_ROOT=<subtree> -- ./scripts/measure/backpressure.sh [rate ...]
# Exit codes: 0 measured, 1 it could not be.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

say() { printf '%s\n' "$*"; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so nothing can be measured"
    exit 1
fi

if [ -z "${BIJECTIVE_CGROUP_ROOT:-}" ]; then
    say "NOT MEASURED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              an unbounded run is not the run a Client gets"
    exit 1
fi

if ! out="$(cd "${CORE}/crates/strategy_engine" && cargo build --quiet --offline --examples 2>&1)"; then
    say "FAIL  the Strategy Engine's examples did not build"
    printf '%s\n' "$out" | tail -3
    exit 1
fi

MEASURE="${CORE}/target/debug/examples/measure_backpressure"
[ -x "$MEASURE" ] || { say "FAIL  ${MEASURE} is missing after a successful build"; exit 1; }

say "MACHINE  $(nproc) cpu, $(uname -sr)"
"$MEASURE" "$@"
