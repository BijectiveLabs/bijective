#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Measures what the stepwise and whole exchanges cost on a window. Asserts both finish and admit.
# Usage: systemd-run --user --scope --setenv=BIJECTIVE_CGROUP_ROOT=<subtree> -- ./scripts/measure/exchange_cost.sh [window ...]
# Exit codes: 0 both exchanges carried every window, 1 one of them did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

WINDOWS=("$@")
[ ${#WINDOWS[@]} -gt 0 ] || WINDOWS=(1000 4000 16000)

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so nothing can be measured"
    exit 1
fi

for target in crates/serialization crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
if ! out="$(cd "${CORE}/crates/strategy_engine" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the Strategy Engine did not build"
    printf '%s\n' "$out" | tail -3
    exit 1
fi

SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
STRATEGY="${CORE}/target/debug/examples/turn_taker"
for binary in "$SUPERVISOR" "$STRATEGY"; do
    [ -x "$binary" ] || { say "FAIL  ${binary} is missing after a successful build"; exit 1; }
done

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT MEASURED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this will not guess one, and an unbounded run is not the run a Client gets"
    exit 1
fi

CGROUP="${CGROUP_ROOT}/bijective-measure-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}" 2>/dev/null || true' EXIT

if ! printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null; then
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"
    exit 1
fi

WALL_SECONDS="$(grep -A2 'pub fn conservative_wall' "${CORE}/crates/strategy_engine/src/lib.rs" \
    | grep -o 'from_secs([0-9]*)' | grep -o '[0-9]*')"
[ -n "$WALL_SECONDS" ] || { say "FAIL  the run's wall bound could not be read from the source"; exit 1; }

say "MACHINE  $(nproc) cpu, $(uname -sr), wall bound ${WALL_SECONDS}s per run"
say ""
printf '%-10s %-9s %10s %12s %14s\n' "window" "exchange" "seconds" "events/s" "fits in wall"
say "--------------------------------------------------------------"

# Runs one window one way and prints the row. Sets `elapsed_ms` for the caller.
elapsed_ms=0
carry() { # <store> <window> <exchange>
    local store="$1" window="$2" exchange="$3" report started ended rate fits
    started="$(date +%s%N)"
    report="$("$SUPERVISOR" \
        --readiness "${WORK}/readiness.json" \
        --cgroup-root "$CGROUP" \
        --store "$store" \
        --strategy "$STRATEGY" \
        --market "market-drill" \
        --to-position "$((window - 1))" \
        --exchange "$exchange" 2>&1)"
    ended="$(date +%s%N)"
    elapsed_ms=$(( (ended - started) / 1000000 ))

    # A rate for a run that admitted nothing is a rate for a run that did not carry the window.
    if ! printf '%s' "$report" | grep -q "1 proposal(s) admitted, 0 refused"; then
        fail "${window} events ${exchange}: the run did not carry the window"
        printf '%s\n' "$report" | tail -3
        return 1
    fi

    if [ "$elapsed_ms" -gt 0 ]; then
        rate=$(( window * 1000 / elapsed_ms ))
    else
        rate=0
    fi
    if [ "$elapsed_ms" -lt $((WALL_SECONDS * 1000)) ]; then fits="yes"; else fits="no"; fi
    printf '%-10s %-9s %10s %12s %14s\n' \
        "$window" "$exchange" "$(printf '%d.%03d' $((elapsed_ms / 1000)) $((elapsed_ms % 1000)))" \
        "$rate" "$fits"
}

for window in "${WINDOWS[@]}"; do
    store="${WORK}/store-${window}"
    mkdir -p "${store}/run"
    if ! seeded="$(cd "${CORE}/crates/serialization" \
        && cargo run --quiet --offline --example seed_market -- "$store" "btc-perp" "$window" 2>&1)"; then
        fail "${window} events could not be seeded"
        printf '%s\n' "$seeded" | tail -3
        continue
    fi

    whole_ms=0
    carry "$store" "$window" whole && whole_ms=$elapsed_ms
    step_ms=0
    carry "$store" "$window" stepwise && step_ms=$elapsed_ms

    # Subtracts what both pay (read, spawn, ledger write) to isolate the exchange's own price.
    if [ "$whole_ms" -gt 0 ] && [ "$step_ms" -gt 0 ]; then
        say "          stepwise costs $((step_ms - whole_ms))ms more over ${window} events" \
            "($(( (step_ms - whole_ms) * 1000 / window ))us per event)"
        if [ "$step_ms" -gt "$whole_ms" ]; then
            say "          at that price the wall bound is reached at about" \
                "$(( window * WALL_SECONDS * 1000 / step_ms )) events"
        fi
    fi
    say ""
done

if [ "$status" -eq 0 ]; then
    say "MEASURED  both exchanges carried every window inside the wall bound"
else
    say "NOT CARRIED  a window above did not complete; the rates above it are what a Client would see"
fi
exit "$status"
