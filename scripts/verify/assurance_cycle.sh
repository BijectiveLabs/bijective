#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Verifies the cost-floor check end to end: profile parses, unread venue recorded as not observed, no fee rests on it,
# and the kit installs the check enabling none of it. Confirming half lives in adapter tests.
# Exit codes: 0 the cycle holds, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the cycle is UNVERIFIED"
    exit 1
fi
if ! out="$(cd "${CORE}/crates/venue_assurance" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the runner did not build"
    printf '%s\n' "$out" | tail -3
    exit 1
fi

RUNNER="${CORE}/target/debug/bijective-verify-venue"
[ -x "$RUNNER" ] || { say "FAIL  ${RUNNER} is missing after a successful build"; exit 1; }

PROFILE="${CORE}/crates/deployment/profiles/example.json"
VENUE="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["venue"])' "$PROFILE")"
mkdir -p "${WORK}/store"
printf 'not an answer this build can read\n' > "${WORK}/answer.json"

ran="$("$RUNNER" "${WORK}/store" "$VENUE" "$PROFILE" BTCUSDT "${WORK}/answer.json" 2>&1)"
say "RUN   ${ran}"
if printf '%s' "$ran" | grep -q 'the profile could not be read'; then
    fail "the shipped profile document does not parse: ${ran}"
else
    say "PASS  the shipped profile document parses"
fi

if printf '%s' "$ran" | grep -q 'recorded NotObserved'; then
    say "PASS  a venue that could not be read is recorded as not observed"
else
    fail "the unobserved result was not recorded: ${ran}"
fi

refused="$(cd "${CORE}/crates/venue_assurance" \
    && cargo run --quiet --offline --example declare_fees -- \
        "${WORK}/store" "$VENUE" 55000 -15000 "a fixture for this drill" 1 2>&1)"
if printf '%s' "$refused" | grep -q 'may not rest on it'; then
    say "PASS  a fee may not rest on a check that could not run"
else
    fail "a fee rested on a check that never observed anything: ${refused}"
fi

units="$(cd "${CORE}/crates/deployment" && cargo test --quiet --offline \
    --test shipped_manifest the_shipped_manifest_installs_the_check_and_schedules_nothing 2>&1)"
if printf '%s' "$units" | grep -q 'test result: ok'; then
    say "PASS  the kit installs the check as a template and enables no instance"
else
    fail "the shipped kit does not install the check: $(printf '%s' "$units" | tail -3)"
fi

if [ "$status" = 0 ]; then
    say "PASS  a check reaches the record, and a fee rests only on one that ran"
fi
exit "$status"
