#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Live strategy end to end: resumes at the present, is told what it missed, proposals reach the stream while running.
# Also checks the session ledger entry, paper output off the Control Engine's stream and decision events.
# Needs a delegated cgroup v2 subtree (see scripts/verify/sandbox.sh). Exit codes: 0 the live lifetime holds, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the live lifetime is UNVERIFIED"
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
    say "FAIL  the Strategy Engine did not build, so the live lifetime is UNVERIFIED"
    printf '%s\n' "$out" | tail -3
    exit 1
fi

SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
[ -x "$SUPERVISOR" ] || { say "FAIL  ${SUPERVISOR} is missing after a successful build"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run"

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the live lifetime is unproven until it runs"
    exit 1
fi

CGROUP="${CGROUP_ROOT}/bijective-live-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}" 2>/dev/null || true' EXIT
if ! printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null; then
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"
    exit 1
fi

seed() { # <count>
    (cd "${CORE}/crates/serialization" \
        && cargo run --quiet --offline --example seed_market -- "${WORK}/store" "btc-perp" "$1" >/dev/null 2>&1)
}
runs() {
    (cd "${CORE}/crates/strategy_engine" \
        && cargo run --quiet --offline --example read_runs -- "${WORK}/store" 2>/dev/null)
}

proposals() {
    (cd "${CORE}/crates/strategy_engine" \
        && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" proposals 2>/dev/null)
}
live_for() { # <seconds> [stage]
    "$SUPERVISOR" \
        --readiness "${WORK}/run/strategy-engine.json" \
        --cgroup-root "${CGROUP}" \
        --store "${WORK}/store" \
        --strategy "${WORK}/strategy.sh" \
        --market "market-drill" \
        --stage "${2:-live}" \
        --drill-seconds "$1" 2>&1
}
on_stream() { # <stream>
    (cd "${CORE}/crates/strategy_engine" \
        && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" "$1" 2>/dev/null)
}

# The strategy reports what it was told, not what it inferred, so what reached it is read from the stream.
cat > "${WORK}/strategy.sh" <<'STRATEGY'
#!/bin/sh
emit() {
  printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"%s","decided_at_ns":"1"}\n' "$1"
}
n=0
while IFS= read -r line; do
  case "$line" in
    *'"event":"resumed"'*)
      case "$line" in
        *not_recorded*) emit "told-nothing-recorded" ;;
        *) emit "told-a-gap" ;;
      esac
      ;;
    *'"event":"book"'*)
      n=$((n+1))
      emit "seen-${n}"
      ;;
  esac
done
STRATEGY
chmod +x "${WORK}/strategy.sh"

seed 3

# Two more arrive while it runs: the only way to tell a tail from a window.
first="$(live_for 5 & sleep 2; seed 2; wait)"
say "S1    ${first}"
after_first="$(proposals)"

if printf '%s' "$after_first" | grep -q 'told-nothing-recorded'; then
    say "PASS  a first session is told the gap is not recorded, rather than that there was none"
else
    fail "the strategy was not told where it resumed: $(printf '%s' "$after_first" | head -3)"
fi

if printf '%s' "$first" | grep -q 'resumed at 3'; then
    say "PASS  the session resumed at the present, not at the beginning"
else
    fail "the session did not resume at the present: ${first}"
fi

seen="$(printf '%s' "$after_first" | grep -c 'seen-')"
if [ "$seen" = "2" ]; then
    say "PASS  only what was recorded while it ran reached it (2 of 5 on the stream)"
else
    fail "the strategy saw ${seen} book event(s), and two were recorded while it ran"
fi

# Gap while nothing listens: the next session must name it.
seed 4

second="$(live_for 3)"
say "S2    ${second}"

if printf '%s' "$second" | grep -q 'resumed at 9 after a gap from 5'; then
    say "PASS  the second session named the gap exactly (positions 5 through 8)"
else
    fail "the gap was not named as it happened: ${second}"
fi

if printf '%s' "$(proposals)" | grep -q 'told-a-gap'; then
    say "PASS  the strategy itself was told there was a gap"
else
    fail "the second session did not tell its strategy about the gap"
fi

ledger="$(runs)"
if [ "$(printf '%s' "$ledger" | grep -c ' live ')" = "2" ]; then
    say "PASS  both sessions left an entry on the run ledger, each named live"
else
    fail "the sessions are not accounted for: $(printf '%s' "$ledger" | head -4)"
fi

# 6: same strategy at the paper stage. Where its proposals land is the check.
before_paper="$(on_stream proposals | head -1)"
seed 2
paper="$(live_for 4 paper & sleep 2; seed 2; wait)"
say "PAPER ${paper}"
after_paper="$(on_stream proposals | head -1)"

if printf '%s' "$paper" | grep -q 'admitted to proposals-paper'; then
    say "PASS  a paper session writes where the Control Engine does not read"
else
    fail "the paper session wrote to the judged stream: ${paper}"
fi

if [ "$before_paper" = "$after_paper" ]; then
    say "PASS  the stream the Control Engine judges did not move (${after_paper})"
else
    fail "a paper session's proposals reached the judged stream: ${before_paper} then ${after_paper}"
fi

if printf '%s' "$(on_stream proposals-paper)" | grep -qE '^[1-9][0-9]* proposal record'; then
    say "PASS  and they are on the paper stream, where a report can read them"
else
    fail "the paper session's proposals are nowhere: $(on_stream proposals-paper | head -2)"
fi

# 7: a decision claimed on an event from an earlier batch.
cat > "${WORK}/backdated.sh" <<'STRATEGY'
#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"event":"book"'*)
      printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c1","decided_at_ns":"1","decided_at_position":"0"}\n'
      ;;
  esac
done
STRATEGY
chmod +x "${WORK}/backdated.sh"
seed 2
backdated="$("$SUPERVISOR" \
    --readiness "${WORK}/run/strategy-engine.json" \
    --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" \
    --strategy "${WORK}/backdated.sh" \
    --market "market-drill" \
    --stage paper \
    --drill-seconds 4 2>&1 & sleep 2; seed 2; wait)"
# Match the reason, not the word: every summary line ends in N refused, so grepping refused alone always passes.
if printf '%s' "$backdated" | grep -q 'refused: the decision claims market position'; then
    say "PASS  a decision claimed on an event already answered is refused and named"
else
    say "      $(printf '%s' "$backdated" | head -2)"
    fail "a backdated decision was admitted: ${backdated}"
fi

# No default stage: a defaulted one could start a session whose proposals reach a venue no configuration selected.
unstaged="$("$SUPERVISOR" \
    --readiness "${WORK}/run/strategy-engine.json" \
    --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" \
    --strategy "${WORK}/strategy.sh" \
    --market "market-drill" \
    --drill-seconds 1 2>&1)"
if printf '%s' "$unstaged" | grep -q 'needs --stage'; then
    say "PASS  a live session with no stage declared is refused"
else
    fail "a session started at an unset stage: ${unstaged}"
fi

# The lag tolerance is the operator's, so a session states it and whether anyone chose it. A set 1,024 and an unset one differ.
undeclared="$(live_for 1 paper)"
if printf '%s' "$undeclared" | grep -q 'tolerating 1024 event(s) behind, a bound not declared'; then
    say "PASS  a session says how far behind it will tolerate, and that the bound was not declared"
else
    fail "the session did not report an undeclared tolerance: ${undeclared}"
fi

declared="$("$SUPERVISOR" \
    --readiness "${WORK}/run/strategy-engine.json" \
    --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" \
    --strategy "${WORK}/strategy.sh" \
    --market "market-drill" \
    --stage paper \
    --events-behind 64 \
    --drill-seconds 1 2>&1)"
if printf '%s' "$declared" | grep -q 'tolerating 64 event(s) behind, declared'; then
    say "PASS  and a declared tolerance reaches the session and is reported as declared"
else
    fail "a declared tolerance did not reach the session: ${declared}"
fi

if [ "$status" = 0 ]; then
    say "PASS  a live strategy resumes at the present and is told what it missed"
fi
exit "$status"
