#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# An operator's strategy end to end: it reads only its window. A cut-short run admits nothing and both runs reach the ledger.
# Run inside a delegated cgroup v2 subtree (see scripts/verify/sandbox.sh): systemd-run --user --scope --setenv=BIJECTIVE_CGROUP_ROOT=<subtree> -- ./scripts/verify/strategy_path.sh
# Exit codes: 0 the join holds, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the join is UNVERIFIED"
    exit 1
fi

for target in crates/serialization crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build, so the store cannot be seeded or read"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
if ! out="$(cd "${CORE}/crates/strategy_engine" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the Strategy Engine did not build, so the join is UNVERIFIED"
    printf '%s\n' "$out" | tail -3
    exit 1
fi

SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
[ -x "$SUPERVISOR" ] || { say "FAIL  ${SUPERVISOR} is missing after a successful build"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run"

# The host must be able to bound a run. Otherwise the supervisor refuses to start and the drill has nothing to say.
CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the join is unproven until it runs"
    exit 1
fi

# Its own subtree under the operator's, so a drill leaves the delegated tree as it found it.
CGROUP="${CGROUP_ROOT}/bijective-drill-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}" 2>/dev/null || true' EXIT

# A cgroup hands a controller to its children only where asked. Without it the run's memory bound cannot be written.
if ! printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null; then
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"
    exit 1
fi

seeded="$(cd "${CORE}/crates/serialization" \
    && cargo run --quiet --offline --example seed_market -- "${WORK}/store" "btc-perp" 6 2>&1)"
say "SEED  ${seeded}"

# Counts book lines sent so a short window shows as a number. The extra printed line must be refused, not passed over.
cat > "${WORK}/strategy.sh" <<'STRATEGY'
#!/bin/sh
n=0
while IFS= read -r line; do
  case "$line" in
    *'"event":"book"'*) n=$((n+1)) ;;
    *end_of_window*) break ;;
  esac
done
printf 'a strategy that logged where it should not\n'
printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"%s","client_id":"c1","decided_at_ns":"1"}\n' "$n"
STRATEGY
chmod +x "${WORK}/strategy.sh"

run_supervisor() { # <strategy> <to-position>
    "$SUPERVISOR" \
        --readiness "${WORK}/run/strategy-engine.json" \
        --cgroup-root "${CGROUP}" \
        --store "${WORK}/store" \
        --strategy "$1" \
        --market "market-drill" \
        --to-position "$2" \
        --exchange whole 2>&1
}

out="$(run_supervisor "${WORK}/strategy.sh" 3)"
say "RUN   ${out}"

# Same reader for both, so a stream nothing can read fails the first check before it passes the second.
on_stream() { # <stream>
    cd "${CORE}/crates/strategy_engine" \
        && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" "$1" 2>/dev/null \
        | sed -n '1s/ proposal record.*//p'
}

priced="$(on_stream "proposals-research")"
priced="${priced:-0}"
if printf '%s' "$out" | grep -q '4 event(s) sent'; then
    say "PASS  the strategy was sent its window, and only its window (4 of 6 seeded)"
else
    fail "the window was not the four declared: ${out}"
fi

if printf '%s' "$out" | grep -q '1 proposal(s) admitted' && [ "$priced" -ge 1 ]; then
    say "PASS  its proposal is on the research stream, readable (${priced} record(s))"
else
    fail "no proposal reached the research stream: ${out}; ${priced} record(s) there"
fi

# 2b: not on the stream the Control Engine reads: it walks its stream from a cursor and ignores where an entry came from.
# The name is a literal on purpose: pointing the research writer back at it must fail this drill.
live_stream="$(on_stream "proposals")"
live_stream="${live_stream:-0}"
if [ "$live_stream" = "0" ]; then
    say "PASS  a backtest's proposals are not on the stream the Control Engine reads"
else
    fail "a bounded run put ${live_stream} proposal(s) on the stream that reaches a venue"
fi

if printf '%s' "$out" | grep -q '1 refused'; then
    say "PASS  a line the supervisor could not read was refused and named"
else
    fail "an unreadable line was passed over rather than refused: ${out}"
fi

# 4: a cut-short run emits a well-formed proposal then fails, so only the exit status stands between it and the stream.
# A crash, not a hang: the same rule, without waiting out a wall bound.
cat > "${WORK}/crashes.sh" <<'CRASHES'
#!/bin/sh
printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"9","client_id":"c2","decided_at_ns":"1"}\n'
exit 1
CRASHES
chmod +x "${WORK}/crashes.sh"

runs() {
    (cd "${CORE}/crates/strategy_engine" \
        && cargo run --quiet --offline --example read_runs -- "${WORK}/store" 2>/dev/null)
}

records() {
    local n
    n="$(on_stream "proposals-research")"
    printf '%s' "${n:-0}"
}
before="$(records)"
cut_short="$(run_supervisor "${WORK}/crashes.sh" 3)"
after="$(records)"

# Two conditions: a supervisor that never started admits nothing too, and a reused cgroup name once hid a run that never began.
if [ "$before" != "$after" ]; then
    fail "a partial run reached the proposal stream: ${before} before, ${after} after"
    say  "      ${cut_short}"
elif ! printf '%s' "$cut_short" | grep -q 'is not a result'; then
    fail "the run did not reach the rule under test: ${cut_short}"
else
    say "PASS  a run that was cut short admitted nothing (${before} proposal record(s) either side)"
fi

ledger="$(runs)"
if printf '%s' "$ledger" | grep -q '^2 run(s) recorded, 1 admission'; then
    say "PASS  both runs are on the ledger with the one admission between them"
else
    fail "the ledger does not hold both runs: $(printf '%s' "$ledger" | head -3)"
fi

if [ "$(printf '%s' "$ledger" | grep -c 'research')" = "2" ]; then
    say "PASS  each entry names the class the run was declared under"
else
    fail "the entries do not name their class: $(printf '%s' "$ledger" | head -3)"
fi

if [ "$status" = 0 ]; then
    say "PASS  a strategy reads its window and its proposals reach the stream"
fi
exit "$status"
