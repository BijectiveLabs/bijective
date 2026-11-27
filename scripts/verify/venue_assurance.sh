#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# The adapter contract conformance suite and venue assurance verifier against a registered venue (runbook 08). Not a change gate, as it reaches an exchange.
# Venue and endpoints are asked of the build, not restated, so they cannot drift from the registry.
# Exit codes: 0 every check that could run passed, 1 otherwise.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

say() { printf '%s\n' "$*"; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH; the drill is UNVERIFIED"; exit 1; }

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
[ -n "$VENUE" ] || { say "FAIL  the registry lists no venue"; exit 1; }

ADAPTER="${CORE}/crates/adapters/${VENUE}"
[ -d "$ADAPTER" ] || { say "FAIL  the registry lists ${VENUE} and no adapter is built for it"; exit 1; }

STREAM="${BIJECTIVE_STREAM_ENDPOINT:-}"
REQUEST="${BIJECTIVE_REQUEST_ENDPOINT:-}"
INSTRUMENT="${BIJECTIVE_INSTRUMENT:-}"
if [ -z "$STREAM" ] || [ -z "$REQUEST" ] || [ -z "$INSTRUMENT" ]; then
        # Refused, not defaulted: a guessed endpoint points the drill at a venue we do not trade.
    say "FAIL  set BIJECTIVE_STREAM_ENDPOINT, BIJECTIVE_REQUEST_ENDPOINT and BIJECTIVE_INSTRUMENT"
    say "      from the venue's profile; this script will not guess them"
    exit 1
fi

out="$(cd "$ADAPTER" && cargo run --quiet --offline --example conformance_probe -- \
        "$STREAM" "$INSTRUMENT" "$REQUEST" 2>&1)"
outcome=$?

printf '%s\n' "$out" | sed 's/^/        /'

if [ "$outcome" = 0 ]; then
    say "PASS  the adapter honoured its contract and the venue still behaves as declared"
else
    say "FAIL  see the findings above; a capital-relevant divergence holds trading on this venue"
fi
exit "$outcome"
