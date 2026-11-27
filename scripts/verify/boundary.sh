#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Boundary check: the core starts, ingests and runs with the opt-in programs stopped.
# Real programs, store and recorded venue traffic. The venue is not live so an outage cannot blind the gate.
# Exit codes: 0 the core came up without the operational host, 1 it did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DRILL_SECONDS="${BOUNDARY_DRILL_SECONDS:-8}"
READY_TIMEOUT="${BOUNDARY_READY_TIMEOUT:-45}"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the programs cannot be built and the boundary is UNVERIFIED"
    exit 1
fi

# Built here, not assumed: a stale binary proves the boundary for a build that is not shipped.
# Venue and recorded traffic come from the build. The adapter contract keeps venue names out of scripts.
VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
if [ -z "$VENUE" ]; then
    say "FAIL  the registry lists no venue, so the core has nothing to ingest from"
    exit 1
fi

FIXTURE="$(find "${CORE}/crates/adapters/${VENUE}/fixtures" -name '*.jsonl' 2>/dev/null | sort | head -1)"
if [ -z "$FIXTURE" ] || [ ! -r "$FIXTURE" ]; then
    say "FAIL  no recorded traffic for the registered venue; the core cannot be shown to ingest"
    exit 1
fi

for crate in crates/data_engine crates/control_engine crates/deployment; do
    if ! out="$(cd "${CORE}/${crate}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${crate} did not build, so the boundary is UNVERIFIED"
        printf '%s\n' "$out" | sed 's/^/        /' | head -5
        exit 1
    fi
done

DATA_ENGINE="${CORE}/target/debug/bijective-data-engine"
CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"
OPERATIONAL_HOST="${CORE}/target/debug/bijective-operational-host"

for binary in "$DATA_ENGINE" "$CONTROL_ENGINE" "$OPERATIONAL_HOST"; do
    [ -x "$binary" ] || { say "FAIL  ${binary} is missing after a successful build"; exit 1; }
done

mkdir -p "${WORK}/store" "${WORK}/run"

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"

# The instruments the recording carries. Fewer is refused at the first unidentifiable frame.
SYMBOLS="$(grep -o '\\"s\\":\\"[A-Z0-9]*\\"' "$FIXTURE" | sed 's/.*:\\"//; s/\\"//' | sort -u | paste -sd, -)"
[ -n "$SYMBOLS" ] || { say "FAIL  the recording names no instrument, so no profile can declare one"; exit 1; }
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "$SYMBOLS"

# The operational host is not started. That absence is the check.
"$DATA_ENGINE" \
    --store "${WORK}/store" \
    --readiness "${WORK}/run/data-engine.json" \
    --venue "$VENUE" \
    --profile "${WORK}/profile.json" \
    --recording "${FIXTURE}" \
    --drill-seconds "$DRILL_SECONDS" >"${WORK}/data-engine.log" 2>&1 &
data_pid=$!

CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --mode live \
    --operators "${WORK}/operators.json" \
    --config "${WORK}/config.json" \
    --keystore "${WORK}/keystore.bin" \
    --credential drill-cred \
    --store "${WORK}/store" \
    --readiness "${WORK}/run/control-engine.json" \
    --drill-seconds "$DRILL_SECONDS" >"${WORK}/control-engine.log" 2>&1 &
control_pid=$!

# Proof by rejection: when set, the host starts beside the core and the check must FAIL.
if [ "${BOUNDARY_PROVE_REFUSAL:-0}" = 1 ]; then
    "$OPERATIONAL_HOST" --store "${WORK}/store" \
        --readiness "${WORK}/run/operational-host.json" \
        --drill-seconds "$DRILL_SECONDS" >/dev/null 2>&1 &
fi

# Readiness is polled, never inferred from the process being alive.
ready() {
    local doc="$1"
    [ -r "$doc" ] || return 1
    grep -q '"health": "ready"' "$doc"
}

waited=0
while [ "$waited" -lt "$READY_TIMEOUT" ]; do
    if ready "${WORK}/run/data-engine.json" && ready "${WORK}/run/control-engine.json"; then
        break
    fi
    sleep 1
    waited=$((waited + 1))
done

ready "${WORK}/run/data-engine.json" \
    || fail "the Data Engine did not report ready within ${READY_TIMEOUT}s"
ready "${WORK}/run/control-engine.json" \
    || fail "the Control Engine did not report ready within ${READY_TIMEOUT}s"

# An absent readiness document is a failure, not a skip: a program that died early must not pass.
if [ -r "${WORK}/run/data-engine.json" ]; then
    ingested="$(grep -o 'ingested [0-9]* book event' "${WORK}/run/data-engine.json" | grep -o '[0-9]*' || true)"
    if [ -z "$ingested" ] || [ "$ingested" -lt 1 ]; then
        fail "the Data Engine reported no ingested events"
    else
        say "PASS  the Data Engine ingested ${ingested} book event(s) with the operational host stopped"
    fi
else
    fail "the Data Engine never wrote a readiness document, so ingestion is UNVERIFIED"
fi

segments="$(find "${WORK}/store" -name '*.seg' -size +0c 2>/dev/null | wc -l)"
[ "$segments" -ge 1 ] || fail "nothing was appended to the store"

if [ -r "${WORK}/run/control-engine.json" ]; then
    # Case-insensitive: the standing line is operator prose, not an interface.
    if grep -qi 'trading halted' "${WORK}/run/control-engine.json"; then
        say "PASS  the Control Engine came up halted, with limit state verified"
    else
        fail "the Control Engine did not come up halted"
    fi
else
    fail "the Control Engine never wrote a readiness document, so its state is UNVERIFIED"
fi

[ -e "${WORK}/run/operational-host.json" ] \
    && fail "the operational host reported readiness, so it was running and the check proves nothing"

wait "$data_pid" 2>/dev/null; data_exit=$?
wait "$control_pid" 2>/dev/null; control_exit=$?
[ "$data_exit" = 0 ] || fail "the Data Engine exited ${data_exit}"
[ "$control_exit" = 0 ] || fail "the Control Engine exited ${control_exit}"

if [ "$status" = 0 ]; then
    say "PASS  core starts, ingests and runs with the opt-in programs stopped"
else
    say
    say "The core did not come up without the operational side."
    say "The core-runs-alone rule is a one-way dependency; a core that needs the operational host does not have one."
    sed 's/^/        data-engine: /' "${WORK}/data-engine.log" | head -5
    sed 's/^/        control-engine: /' "${WORK}/control-engine.log" | head -5
fi

exit "$status"
