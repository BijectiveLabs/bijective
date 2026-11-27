#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Restore drill: builds a real store, runs the drill and requires both of its proofs to be reported.
# The drill proves its own refusals. This script makes sure it is invoked at all.
# Exit codes: 0 the restore verified and the refusals were demonstrated, 1 otherwise.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KIT="${CORE}/crates/deployment"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

say() { printf '%s\n' "$*"; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH; the drill is UNVERIFIED"; exit 1; }

for crate in crates/data_engine crates/deployment; do
    if ! out="$(cd "${CORE}/${crate}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${crate} did not build, so the drill is UNVERIFIED"
        printf '%s\n' "$out" | sed 's/^/        /' | head -5
        exit 1
    fi
done

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
FIXTURE="$(find "${CORE}/crates/adapters/${VENUE}/fixtures" -name '*.jsonl' 2>/dev/null | sort | head -1)"
[ -n "$FIXTURE" ] || { say "FAIL  no recorded traffic to build a store from"; exit 1; }

mkdir -p "${WORK}/store"
# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
SYMBOLS="$(grep -o '\\"s\\":\\"[A-Z0-9]*\\"' "$FIXTURE" | sed 's/.*:\\"//; s/\\"//' | sort -u | paste -sd, -)"
[ -n "$SYMBOLS" ] || { say "FAIL  the recording names no instrument, so no profile can declare one"; exit 1; }
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "$SYMBOLS"

"${CORE}/target/debug/bijective-data-engine" \
    --store "${WORK}/store" --readiness "${WORK}/de.json" \
    --venue "$VENUE" --profile "${WORK}/profile.json" \
    --recording "$FIXTURE" --drill-seconds 0 >"${WORK}/de.log" 2>&1 \
    || { say "FAIL  could not build a store to restore"; sed 's/^/        /' "${WORK}/de.log" | head -5; exit 1; }

report="$("${CORE}/target/debug/bijective-deploy" restore-drill "${WORK}/store" "${WORK}/restored" 2>&1)"
outcome=$?

printf '%s\n' "$report" | sed 's/^/        /'

if [ "$outcome" != 0 ]; then
    say "FAIL  the restore drill did not pass on a store it had just built"
    exit 1
fi

# Demonstrated, not merely available: a drill that stops proving itself still passes every restore.
proofs="$(printf '%s\n' "$report" | grep -c '^PROVEN')"
if [ "$proofs" -lt 2 ]; then
    say "FAIL  the drill reported ${proofs} proof(s); it must demonstrate both refusals every run"
    exit 1
fi

say "PASS  restore verified and both refusals demonstrated"
exit 0
