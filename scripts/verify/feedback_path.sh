#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Order feedback end to end: a live strategy proposes, the Control Engine answers, and the answer
# reaches the strategy that proposed it as an `order` event, with nothing but the two engines and
# the store between them. No reachable venue by design: the Control Engine comes up halted and
# refuses, and a refusal told back is still an answer told back.
# Needs a delegated cgroup v2 subtree (see scripts/verify/sandbox.sh). Exit codes: 0 the answer
# reached the strategy, 1 it did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the feedback path is UNVERIFIED"
    exit 1
fi

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
if [ -z "$VENUE" ]; then
    say "FAIL  the build registers no venue, so no configuration can name one"
    exit 1
fi

# Built here, not assumed: a stale binary proves the path for a build that is not shipped.
for target in crates/control_engine crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build, so the feedback path is UNVERIFIED"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
for target in crates/serialization crates/control_engine crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build, so the path cannot be seeded or read"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"
SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
SEED="${CORE}/target/debug/examples/seed_market"
for program in "$CONTROL_ENGINE" "$SUPERVISOR" "$SEED"; do
    [ -x "$program" ] || { say "FAIL  ${program} is missing after a successful build"; exit 1; }
done

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the feedback path is unproven until it runs"
    exit 1
fi
CGROUP="${CGROUP_ROOT}/bijective-feedback-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}" 2>/dev/null || true' EXIT
if ! printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null; then
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"
    exit 1
fi

mkdir -p "${WORK}/store" "${WORK}/run"
# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "BTCUSDT"
# A desk limit and a limit on the strategy below, by the name the supervisor runs it under (its
# file's stem): the node must start with both in force, and the strategy's proposals carry it.
sed -i 's|^    { "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "100000000000" }$|&,\n    { "scope": "desk:alpha", "kind": "gross_exposure", "maximum_money_scaled": "50000000000" },\n    { "scope": "strategy:strategy", "kind": "gross_exposure", "maximum_money_scaled": "50000000000" }|' "${WORK}/config.json"
grep -q '"strategy:strategy"' "${WORK}/config.json" || fail "the drill could not declare its desk and strategy limits"

# Proposes once on its first book, then reports the first answer it is told as a second proposal:
# the strategy's own output is the only witness of what reached it.
cat > "${WORK}/strategy.sh" <<STRATEGY
#!/bin/sh
proposed=0; heard=0
while IFS= read -r line; do
  case "\$line" in
    *'"event":"book"'*)
      if [ "\$proposed" -eq 0 ]; then
        proposed=1
        printf '{"emit":"proposal","desk":"alpha","instrument":"${VENUE}.BTCUSDT","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"asked","decided_at_ns":"1"}\n'
      fi ;;
    *'"event":"order"'*)
      if [ "\$heard" -eq 0 ]; then
        heard=1
        told=\$(printf '%s' "\$line" | sed 's/.*"status":"\([a-z_]*\)".*/\1/')
        printf '{"emit":"proposal","desk":"alpha","instrument":"${VENUE}.BTCUSDT","side":"buy","quantity_scaled":"1","price_scaled":"1","client_id":"heard-%s","decided_at_ns":"1"}\n' "\$told"
      fi ;;
  esac
done
STRATEGY
chmod +x "${WORK}/strategy.sh"

"$SEED" "${WORK}/store" "${VENUE}.BTCUSDT" 1 >/dev/null 2>&1 || fail "the market was not seeded"

# The session resumes at the present, so books arrive while it runs. The Control Engine starts
# after the session, so every answer it gives is one the session can hear.
"$SUPERVISOR" --readiness "${WORK}/run/strategy-engine.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/strategy.sh" --market market-drill \
    --stage live --drill-seconds 14 >"${WORK}/strategy-engine.log" 2>&1 &
session=$!
sleep 2
"$SEED" "${WORK}/store" "${VENUE}.BTCUSDT" 2 >/dev/null 2>&1
CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --mode live \
    --operators "${WORK}/operators.json" \
    --config "${WORK}/config.json" \
    --keystore "${WORK}/keystore.bin" \
    --credential drill-cred \
    --store "${WORK}/store" \
    --readiness "${WORK}/run/control-engine.json" \
    --drill-seconds 8 >"${WORK}/control-engine.log" 2>&1
wait "$session"

asked="$(cd "${CORE}/crates/strategy_engine" \
    && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" proposals 2>/dev/null)"
told="$(cd "${CORE}/crates/serialization" \
    && cargo run --quiet --offline --example read_feedback -- "${WORK}/store" 2>/dev/null)"

if printf '%s' "$told" | grep -q '^proposal 0 refused'; then
    say "PASS  the Control Engine answered the strategy's proposal and told the answer back"
else
    fail "no answer to the first proposal was told back: ${told}"
    tail -5 "${WORK}/control-engine.log" | sed 's/^/      /'
fi
if printf '%s' "$asked" | grep -q 'asked .* desk=alpha strategy=strategy'; then
    say "PASS  the proposal carries the strategy the supervisor ran, so its limit can bind it"
else
    fail "the proposal does not name the strategy that made it: ${asked}"
fi
if grep -q 'limits refused\|was refused' "${WORK}/control-engine.log"; then
    fail "the node refused its desk or strategy limit"
    tail -3 "${WORK}/control-engine.log" | sed 's/^/      /'
fi
if printf '%s' "$asked" | grep -q 'heard-refused'; then
    say "PASS  the strategy that proposed was told the answer, and acted on it"
else
    fail "the strategy did not report hearing its answer: ${asked}"
    tail -5 "${WORK}/strategy-engine.log" | sed 's/^/      /'
fi

if [ "$status" -eq 0 ]; then
    say "PASS  a proposal's answer reaches the strategy that made it"
fi
exit "$status"
