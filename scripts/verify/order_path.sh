#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Order path end to end: a proposal enters, the engine answers it, and a restart does not answer it twice.
# No reachable venue by design: the engine comes up halted and refuses, and a refusal recorded is still an answer.
# Exit codes: 0 the path answered everything once, 1 it did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DRILL_SECONDS="${ORDER_PATH_DRILL_SECONDS:-6}"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the programs cannot be built and the path is UNVERIFIED"
    exit 1
fi

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
if [ -z "$VENUE" ]; then
    say "FAIL  the build registers no venue, so no configuration can name one"
    exit 1
fi

# Built here, not assumed: a stale binary proves the path for a build that is not shipped.
if ! out="$(cd "${CORE}/crates/control_engine" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the Control Engine did not build, so the path is UNVERIFIED"
    printf '%s\n' "$out" | tail -3
    exit 1
fi
for target in crates/serialization crates/control_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build, so the path cannot be seeded or read"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"
[ -x "$CONTROL_ENGINE" ] || { say "FAIL  ${CONTROL_ENGINE} is missing after a successful build"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run"

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "BTCUSDT"

seeded="$(cd "${CORE}/crates/serialization" \
    && cargo run --quiet --offline --example seed_proposals -- \
        "${WORK}/store" "${VENUE}.BTCUSDT" --with-unreadable 2>&1)"
say "SEED  ${seeded}"

run_engine() {
    CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
        --mode live \
        --operators "${WORK}/operators.json" \
        --config "${WORK}/config.json" \
        --keystore "${WORK}/keystore.bin" \
        --credential drill-cred \
        --store "${WORK}/store" \
        --readiness "${WORK}/run/control-engine.json" \
        --drill-seconds "$DRILL_SECONDS" >>"${WORK}/control-engine.log" 2>&1
    return $?
}

# Count answers, not events: a venue withdrawal entry would read as a proposal answered twice.
answered() {
    cd "${CORE}/crates/control_engine" \
        && cargo run --quiet --offline --example read_control_stream -- "${WORK}/store" control 2>/dev/null \
        | grep -c 'from_position'
}

run_engine
first_exit=$?
first="$(answered)"
first="${first:-0}"

# Three in, three out. Fewer is a proposal passed over with the mark moved past it.
if [ "$first" -ge 3 ]; then
    say "PASS  every proposal was answered, including the one this build cannot decode (${first} entries)"
else
    fail "3 proposals went in and ${first} answer(s) came out, so at least one was passed over"
    say "      the engine exited ${first_exit}; its last words follow"
    tail -5 "${WORK}/control-engine.log" | sed 's/^/      /'
fi

# The one that spends money: a resume mark that did not hold re-answers every proposal.
# Asserted on positions, not count, which cannot tell no re-answer from a re-answer after the first record went missing.
run_engine
positions() {
    cd "${CORE}/crates/control_engine" \
        && cargo run --quiet --offline --example read_control_stream -- "${WORK}/store" control 2>/dev/null \
        | grep -oE 'from_position: [0-9]+' | awk '{print $2}'
}
all="$(positions)"
total="$(printf '%s\n' "$all" | grep -c .)"
distinct="$(printf '%s\n' "$all" | sort -u | grep -c .)"

if [ "$total" = "$distinct" ] && [ "$total" -ge 3 ]; then
    say "PASS  each of ${total} position(s) was answered exactly once across two runs"
else
    fail "${total} answer(s) cover ${distinct} position(s), so a proposal was answered more than once"
    printf '%s\n' "$all" | sort | uniq -c | sed 's/^/      /'
fi

# Each answer is mirrored for the strategy that proposed it, once, across both runs.
told="$(cd "${CORE}/crates/serialization" && cargo run --quiet --offline --example read_feedback -- "${WORK}/store" 2>/dev/null)"
told_total="$(printf '%s\n' "$told" | grep -c '^proposal')"
told_distinct="$(printf '%s\n' "$told" | grep '^proposal' | awk '{print $2}' | sort -u | grep -c .)"
if [ "$told_total" -ge 3 ] && [ "$told_total" = "$total" ] && [ "$told_distinct" = "$distinct" ]; then
    say "PASS  every answer was told back once, to the proposal it answered (${told_total} record(s))"
else
    fail "the feedback holds ${told_total} record(s) for ${total} answer(s): ${told}"
fi

# The loop must still ask whether a halt was honoured. Unit tests cannot see a dropped call.
# The engine states what it asked on its readiness surface each pass.
if grep -q 'safe-state postcondition' "${WORK}/run/control-engine.json" 2>/dev/null; then
    say "PASS  the running engine asks for the safe-state postcondition every pass"
else
    fail "the engine's standing report does not mention the safe-state postcondition"
    say "      nothing in the loop is asking whether a halt was honoured"
fi

# 5: the record the real path left behind, read back by the audit an operator runs.
if ! out="$(cd "${CORE}/crates/ordering" && cargo build --quiet --offline 2>&1)"; then
    fail "the ordering audit did not build"
    printf '%s\n' "$out" | tail -3
else
    ordered="$("${CORE}/target/debug/bijective-check-ordering" "${WORK}/store" 2>&1)"
    say "ORD   ${ordered}"
    if printf '%s' "$ordered" | grep -q 'nothing out of order'; then
        say "PASS  the record this path left behind is in order"
    else
        fail "the real order path left a record out of order: ${ordered}"
    fi
    if printf '%s' "$ordered" | grep -qE '^[1-9][0-9]* control record\(s\) read'; then
        say "PASS  and the audit read something, so a clean answer is not an empty one"
    else
        fail "the audit read nothing, so its answer says nothing: ${ordered}"
    fi

        # A clean audit counts only if it could have failed: against an empty proposal stream every act names a missing decision.
    could="$("${CORE}/target/debug/bijective-check-ordering" "${WORK}/store" control proposals-none 2>&1)"
    if printf '%s' "$could" | grep -q 'names proposal'; then
        say "PASS  and it would have said so: the same record against an empty stream reports"
    else
        fail "the audit reports nothing whatever it is given: ${could}"
    fi
fi

if [ "$status" = 0 ]; then
    say "PASS  a proposal enters, is answered once, and a restart does not answer it again"
fi
exit "$status"
