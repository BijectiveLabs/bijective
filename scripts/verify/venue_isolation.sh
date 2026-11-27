#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# venue isolation driven as processes: two venues' Data Engines on one store each get a stream carrying only their own identities.

set -u
status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH, so isolation is UNVERIFIED"; exit 1; }

if ! out="$(cd "$CORE" && cargo build --quiet --offline \
        -p bijective-data-engine \
        -p bijective-venue --example drill_profile 2>&1)"; then
    say "FAIL  the programs did not build, so isolation is UNVERIFIED"
    printf '%s\n' "$out" | sed 's/^/        /' | head -5
    exit 1
fi

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"

DATA_ENGINE="${CORE}/target/debug/bijective-data-engine"
[ -x "$DATA_ENGINE" ] || { say "FAIL  the Data Engine is missing after a successful build"; exit 1; }

FIRST="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | sed -n '1p')"
SECOND="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | sed -n '2p')"
if [ -z "$FIRST" ] || [ -z "$SECOND" ]; then
    say "FAIL  fewer than two venues are registered, so isolation cannot be shown"
    exit 1
fi

FIXTURE="$(find "${CORE}/crates/adapters/${FIRST}/fixtures" -name '*.jsonl' 2>/dev/null | sort | head -1)"
[ -r "$FIXTURE" ] || { say "FAIL  no recorded traffic for ${FIRST}"; exit 1; }
SYMBOLS="$(grep -o '\\"s\\":\\"[A-Z0-9]*\\"' "$FIXTURE" | sed 's/.*:\\"//; s/\\"//' | sort -u | paste -sd, -)"
[ -n "$SYMBOLS" ] || { say "FAIL  the recording names no instrument"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run"

# Frames written here, not kept as a file: a synthetic recording in a fixtures directory gets mistaken for captured traffic.
SECOND_SYMBOL="BTC"
cat > "${WORK}/second.jsonl" <<'FRAMES'
{"at_ns":"1000","frame":"{\"channel\":\"l2Book\",\"data\":{\"coin\":\"BTC\",\"time\":1712345678000,\"levels\":[[{\"px\":\"64000.5\",\"sz\":\"1.5\"}],[{\"px\":\"64001.5\",\"sz\":\"2.0\"}]]}}"}
{"at_ns":"1001000000","frame":"{\"channel\":\"l2Book\",\"data\":{\"coin\":\"BTC\",\"time\":1712345679000,\"levels\":[[{\"px\":\"64002.5\",\"sz\":\"1.0\"}],[{\"px\":\"64003.5\",\"sz\":\"1.0\"}]]}}"}
FRAMES

drill_profile "$CORE" "$FIRST" "$SYMBOLS" "${WORK}/first-profile.json"
drill_profile "$CORE" "$SECOND" "$SECOND_SYMBOL" "${WORK}/second-profile.json"

# Both at once against one store: the writer lock is held for the process's life, so only concurrency exercises the failure.
"$DATA_ENGINE" --store "${WORK}/store" --readiness "${WORK}/run/first.json" \
    --venue "$FIRST" --profile "${WORK}/first-profile.json" \
    --recording "$FIXTURE" --drill-seconds 0 >"${WORK}/first.log" 2>&1 &
first_pid=$!

"$DATA_ENGINE" --store "${WORK}/store" --readiness "${WORK}/run/second.json" \
    --venue "$SECOND" --profile "${WORK}/second-profile.json" \
    --recording "${WORK}/second.jsonl" --drill-seconds 0 >"${WORK}/second.log" 2>&1 &
second_pid=$!

wait "$first_pid"; first_exit=$?
wait "$second_pid"; second_exit=$?

if [ "$first_exit" != 0 ] || [ "$second_exit" != 0 ]; then
    fail "a Data Engine refused to run beside another (${FIRST}=${first_exit}, ${SECOND}=${second_exit})"
    say "    --- ${FIRST} ---"; sed 's/^/        /' "${WORK}/first.log" | head -3
    say "    --- ${SECOND} ---"; sed 's/^/        /' "${WORK}/second.log" | head -3
    say "    profile: $(wc -c < "${WORK}/second-profile.json") bytes"
else
    say "PASS  both venues ingested into one store at the same time"
fi

for venue in "$FIRST" "$SECOND"; do
    if [ -d "${WORK}/store/market-${venue}" ]; then
        say "PASS  ${venue} wrote its own market stream"
    else
        fail "${venue} has no stream of its own; the venues shared one"
    fi
done

# Separable halves: two streams with mixed identities would pass the check above and let one venue's quote price another's position.
for venue in "$FIRST" "$SECOND"; do
    dir="${WORK}/store/market-${venue}"
    [ -d "$dir" ] || continue
    other="$FIRST"; [ "$venue" = "$FIRST" ] && other="$SECOND"
    if grep -aq "${other}\." "$dir"/*.seg 2>/dev/null; then
        fail "${venue}'s stream carries an identity belonging to ${other}"
    else
        say "PROVEN ${venue}'s stream carries no identity from ${other}"
    fi
done

if [ "$status" = 0 ]; then
    say "PASS  two venues run side by side, each in its own stream, neither in the other's"
else
    say ""
    say "One venue's Data Engine could not run beside another's, or their records were mixed."
    say "One writer, one stream, one venue, and the identities stay apart."
fi
exit "$status"
