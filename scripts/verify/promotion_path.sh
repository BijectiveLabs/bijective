#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Verifies a promotion answer from the platform's own record, with real runs, ledger and decision.
# An unrecorded run is not a refusal. Unrepeated, unmeasured, unclaimed and non-reproducing runs are each refused for their own term.
# Needs a delegated cgroup subtree (see scripts/verify/sandbox.sh). Exit codes: 0 the path holds, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the promotion path is UNVERIFIED"
    exit 1
fi

for target in crates/serialization crates/strategy_engine crates/evaluation crates/evaluation crates/venue_assurance; do
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
[ -x "$SUPERVISOR" ] || { say "FAIL  ${SUPERVISOR} is missing after a successful build"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run"

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the path is unproven until it runs"
    exit 1
fi

CGROUP="${CGROUP_ROOT}/bijective-promotion-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}" 2>/dev/null || true' EXIT
if ! printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null; then
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"
    exit 1
fi

# Six updates for a window of four, so a horizon of one position has a book to mark against.
(cd "${CORE}/crates/serialization" \
    && cargo run --quiet --offline --example seed_market -- "${WORK}/store" "btc-perp" 6 >/dev/null 2>&1)

# Takes its turn: a stepwise exchange sends one event and waits. The limit is above the seeded ask so the order crosses.
cat > "${WORK}/steady.sh" <<'STRATEGY'
#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *end_of_window*) break ;;
    *'"event":"book"'*)
      printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"c1","decided_at_ns":"1"}\n'
      printf '{"emit":"answered"}\n'
      ;;
  esac
done
STRATEGY

# Output carries the moment it ran, which only the digest catches.
cat > "${WORK}/restless.sh" <<'STRATEGY'
#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *end_of_window*) break ;;
    *'"event":"book"'*)
      printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"c1","decided_at_ns":"%s"}\n' "$(date +%s%N)"
      printf '{"emit":"answered"}\n'
      ;;
  esac
done
STRATEGY
chmod +x "${WORK}/steady.sh" "${WORK}/restless.sh"

run_once() { # <strategy>
    "$SUPERVISOR" \
        --readiness "${WORK}/run/strategy-engine.json" \
        --cgroup-root "${CGROUP}" \
        --store "${WORK}/store" \
        --strategy "$1" \
        --market "market-drill" \
        --to-position 3 \
        --exchange stepwise 2>&1
}
why_not() { # <prefix> [confidence]
    (cd "${CORE}/crates/evaluation" \
        && cargo run --quiet --offline --example why_not -- "${WORK}/store" "$1" ${2:+"$2"} 2>&1)
}
ledger() {
    (cd "${CORE}/crates/strategy_engine" \
        && cargo run --quiet --offline --example read_runs -- "${WORK}/store" 2>/dev/null)
}

absent="$(why_not "deadbeef")"
if printf '%s' "$absent" | grep -q 'no run recorded here begins with'; then
    say "PASS  a run this node has no entry for is not a decision"
else
    fail "an unrecorded run produced a decision: ${absent}"
fi

run_once "${WORK}/steady.sh" >/dev/null
steady="$(ledger | awk 'NR==2 {print $2}')"
[ -n "$steady" ] || { fail "the run left no ledger entry"; exit 1; }
once="$(why_not "$steady")"
if printf '%s' "$once" | grep -q 'never replayed'; then
    say "PASS  a run that ran once is refused for not having been replayed"
else
    fail "a single run was not refused for that: ${once}"
fi

run_once "${WORK}/steady.sh" >/dev/null
twice="$(why_not "$steady")"
if printf '%s' "$twice" | grep -q 'Identical'; then
    say "PASS  the same specification twice reproduced, read from the ledger"
else
    fail "the replay was not recognised: ${twice}"
fi
if printf '%s' "$twice" | grep -q 'nothing measured this run'; then
    say "PASS  a reproduced run is refused for what nothing measured, not for its claim"
else
    fail "the refusal did not name the term that stopped it: ${twice}"
fi

run_once "${WORK}/restless.sh" >/dev/null
run_once "${WORK}/restless.sh" >/dev/null
restless="$(ledger | grep -v "$steady" | awk 'NR==2 {print $2}')"
[ -n "$restless" ] || { fail "the second strategy left no ledger entry"; exit 1; }
diverged="$(why_not "$restless")"
if printf '%s' "$diverged" | grep -q 'did not reproduce'; then
    say "PASS  a strategy whose output differs between runs is caught by the digest"
else
    fail "a non-deterministic run was not caught: ${diverged}"
fi

# 5: the same run, measured. 19 600 is 1.96 in the fixed scale, and the operator declares it.
measured="$(why_not "$steady" 19600)"
say "M     $(printf '%s' "$measured" | sed -n 2p)"
if printf '%s' "$measured" | grep -qE 'measured: [1-9][0-9]* observation'; then
    say "PASS  the run's proposals were priced against the book its author was sent"
else
    fail "nothing was measured from a run that proposed: ${measured}"
fi

if printf '%s' "$measured" | grep -q 'Unregistered'; then
    say "PASS  a measured run is refused for the unregistered claim, not for the measurement"
else
    fail "the refusal did not move on once the run was measured: ${measured}"
fi

declared="$(cd "${CORE}/crates/evaluation" \
    && cargo run --quiet --offline --example declare_claim -- \
        "${WORK}/store" "${WORK}/steady.sh" 3:stepwise \
        "the move over one position clears what it costs to take it" \
        net_after_spread_bps at-least 25 1 0:3:3:4 2>&1)"
say "R     ${declared}"

judged="$(why_not "$steady" 19600)"
say "C     $(printf '%s' "$judged" | grep horizon)"
if printf '%s' "$judged" | grep -q 'registration on the registry'; then
    say "PASS  the claim declared before the run is the one the decision is made against"
else
    fail "the registration was not read back: ${judged}"
fi

if printf '%s' "$judged" | grep -q 'does not clear'; then
    say "PASS  an edge smaller than the spread it crossed does not clear its cost"
else
    fail "the edge was not compared against its cost: ${judged}"
fi

if printf '%s' "$judged" | grep -q 'NotMet'; then
    say "PASS  the claim is judged on the figure that was measured, not on the measurement's absence"
else
    fail "the claim was not judged against the measured figure: ${judged}"
fi

# 7: market-drill is the stream, so drill is the venue. Only the refusal is drilled: the accepting half needs a venue named
# and lives in the adapter's own tests.
fee() { # <args...>
    (cd "${CORE}/crates/venue_assurance" \
        && cargo run --quiet --offline --example declare_fees -- "$@" 2>&1)
}

unchecked="$(fee "${WORK}/store" drill 55000 -15000 "a fixture for this drill" 1)"
if printf '%s' "$unchecked" | grep -q 'rest on nothing'; then
    say "PASS  a fee for an unverified venue is refused"
else
    fail "a fee resting on nothing was accepted: ${unchecked}"
fi

incomplete="$(why_not "$steady" 19600)"
if printf '%s' "$incomplete" | grep -q 'no fee is declared for drill'; then
    say "PASS  the floor names the venue whose fee is missing, rather than reporting the spread as the cost"
else
    fail "the floor did not name what was missing: ${incomplete}"
fi

held="$(why_not "$steady" 19600)"
if printf '%s' "$held" | grep -q 'basis HeldOut'; then
    say "PASS  the figures are the ones the claim's own structure holds out"
else
    fail "the measurement was not taken on the declared structure: ${held}"
fi

if printf '%s' "$held" | grep -q 'fold 0 tested \[3, 4): 2 observation'; then
    say "PASS  the fold the claim declared is where the observations were counted"
else
    fail "the fold was not measured as declared: $(printf '%s' "$held" | grep fold)"
fi

"$SUPERVISOR" \
    --readiness "${WORK}/run/strategy-engine.json" \
    --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" \
    --strategy "${WORK}/steady.sh" \
    --market "market-drill" \
    --stage paper \
    --drill-seconds 2 >/dev/null 2>&1

both="$(ledger)"
# Taken from its own ledger line rather than recomputed.
mark="$(printf '%s' "$both" | grep "$steady" | grep ' research ' | grep -oE 'strategy=[0-9a-f]+' | head -1)"
if [ -n "$mark" ] && printf '%s' "$both" | grep ' live ' | grep -q "$mark"; then
    say "PASS  one strategy across both classes, and the ledger says which (${mark})"
else
    say "      $(printf '%s' "$both" | grep -E ' (research|live) ' | head -4)"
    fail "the ledger does not carry one strategy across the stages"
fi

staged="$(printf '%s' "$both" | grep "$mark" | grep -E ' (research|live) ' | awk '{print $2}' | sort -u | wc -l)"
if [ "$staged" -ge 2 ]; then
    say "PASS  and they are different runs, which is what a stage being a stage means"
else
    fail "the stages share a run identity: ${staged}"
fi

if [ "$status" = 0 ]; then
    say "PASS  the platform answers why a run may not be promoted, from its own record"
fi
exit "$status"
