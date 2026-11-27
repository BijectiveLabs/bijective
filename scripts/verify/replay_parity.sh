#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Parity claim made falsifiable: emitted stream depends only on the program's clock, its record and its recording.
# Two backtests over identical records and one recording emit byte-identical control streams. Two guards stop a vacuous pass.
# The backtest fills on simulated venues and reads no credential (decision 0015).
# Replay is how a backtest is fed (--replay-from), not a run mode: the modes are backtest, paper and live.
# Exit codes: 0 the program carries no dependence on the host, 1 it does.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DRILL_SECONDS="${REPLAY_PARITY_DRILL_SECONDS:-6}"
REPLAY_AT="${REPLAY_PARITY_AT_NS:-1800000000000000000}"
REPLAY_ELSEWHERE="${REPLAY_PARITY_OTHER_NS:-1900000000000000000}"
UPDATES=20

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so parity is UNVERIFIED"
    exit 1
fi

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
if [ -z "$VENUE" ]; then
    say "FAIL  the build registers no venue, so no configuration can name one"
    exit 1
fi

if ! out="$(cd "${CORE}/crates/control_engine" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the Control Engine did not build, so parity is UNVERIFIED"
    printf '%s\n' "$out" | tail -3
    exit 1
fi
for target in crates/serialization crates/control_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build, so the store cannot be seeded"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"
SEED_MARKET="${CORE}/target/debug/examples/seed_market"
SEED_PROPOSALS="${CORE}/target/debug/examples/seed_proposals"
for program in "$CONTROL_ENGINE" "$SEED_MARKET" "$SEED_PROPOSALS"; do
    [ -x "$program" ] || { say "FAIL  ${program} is missing after a successful build"; exit 1; }
done

mkdir -p "${WORK}/store" "${WORK}/market" "${WORK}/run"

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "BTCUSDT"
INSTRUMENT="${VENUE}.BTCUSDT"
MARKET="market-${VENUE}"
# Room for one order of a few dozen in money, so the seeded buy and sell are admitted and fill.
sed -i 's|"maximum_per_order_scaled": "100000000"|"maximum_per_order_scaled": "100000000000"|' "${WORK}/config.json"

"$SEED_MARKET" "${WORK}/market" "$INSTRUMENT" "$UPDATES" 0 "$VENUE" "$(( REPLAY_AT / 1000000 ))" >/dev/null
seeded="$("$SEED_PROPOSALS" "${WORK}/store" "$INSTRUMENT" --backtest --with-unreadable 2>&1)"
say "SEED  ${seeded}"

# One seeded record, copied: running twice into one record appends to a history the first run changed.
# No keystore, no credential directory: a backtest that needed one would fail here.
run_over() { # <label> <start-ns>
    local label="$1" at="$2"
    rm -rf "${WORK}/${label}"
    cp -a "${WORK}/store" "${WORK}/${label}"
    "$CONTROL_ENGINE" \
        --config "${WORK}/config.json" \
        --store "${WORK}/${label}" \
        --market-store "${WORK}/market" \
        --market "$MARKET" \
        --readiness "${WORK}/run/${label}.json" \
        --mode backtest \
        --replay-from "$at" >>"${WORK}/${label}.log" 2>&1
}

run_over first  "$REPLAY_AT"
run_over second "$REPLAY_AT"
run_over other  "$REPLAY_ELSEWHERE"

control_bytes() { # <label> - the control stream alone, which is what the engine decided
    find "${WORK}/$1" -type f -path '*control-backtest*' ! -name '*.lock' -exec sha256sum {} + 2>/dev/null \
        | awk '{print $1}' | sort | paste -sd, -
}

first_sum="$(control_bytes first)"
second_sum="$(control_bytes second)"
other_sum="$(control_bytes other)"

record="$(cd "${CORE}/crates/control_engine" \
    && cargo run --quiet --offline --example read_control_stream -- "${WORK}/first" control-backtest 2>/dev/null)"

# Empty proves nothing: two runs that decided nothing agree, and the agreement says nothing.
answers="$(printf '%s\n' "$record" | grep -c 'from_position')"
if [ "${answers:-0}" -lt 1 ]; then
    fail "the backtest decided nothing, so identical output proves nothing about the program"
    say  "      it exited having written no answer; its last words follow"
    tail -5 "${WORK}/first.log" | sed 's/^/      /'
else
    say "PASS  the backtest decided something to compare (${answers} answer(s))"
fi

fills="$(printf '%s\n' "$record" | grep -c 'Filled {')"
if [ "${fills:-0}" -ge 2 ] && printf '%s\n' "$record" | grep -q 'Refused .*Unreadable'; then
    say "PASS  the backtest filled ${fills} order(s) on the simulated venue and recorded the unreadable proposal"
else
    fail "the backtest did not fill the seeded buy and sell (${fills:-0} fill(s))"
    tail -5 "${WORK}/first.log" | sed 's/^/      /'
fi

if grep -q 'equity change' "${WORK}/first.log" && grep -q "^${VENUE}: 2 order(s) at the venue, 2 fill(s)" "${WORK}/first.log"; then
    say "PASS  the backtest stated its orders, fills and equity change"
else
    fail "the backtest's result does not state its orders, fills and equity change"
fi

if [ -n "$first_sum" ] && [ "$first_sum" = "$second_sum" ]; then
    say "PASS  two runs over identical records under one clock emitted byte-identical control streams"
else
    fail "the same program over the same input emitted different bytes, so something of the host reached the record"
    say  "      first  ${first_sum:-<nothing>}"
    say  "      second ${second_sum:-<nothing>}"
fi

# A clock the output ignores would make the equality above a property of the program, not of parity.
if [ "$first_sum" != "$other_sum" ]; then
    say "PASS  a different clock produced different bytes, so the clock is genuinely read"
else
    fail "moving the clock changed nothing, so the equality above is not evidence of parity"
fi

# The recording moves the clock: the last pass is stamped at the last update's receipt.
LAST_AT="$(( REPLAY_AT + UPDATES * 1000000 ))"
stamped="$(grep -o '"stamped_at_ns": *"[0-9]*"' "${WORK}/run/first.json" 2>/dev/null | grep -oE '[0-9]+' | tail -1)"
if [ "$stamped" = "$LAST_AT" ]; then
    say "PASS  the run stamped its surface at the recording's last instant (${stamped})"
else
    fail "the backtest stamped ${stamped:-<nothing>}, not the ${LAST_AT} its recording ends at"
fi

# A run that cannot tell real time must not trade: the backtest opens simulated venues only, beside a live run that opens the venue.
CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --config "${WORK}/config.json" \
    --keystore "${WORK}/keystore.bin" \
    --credential drill-cred \
    --store "${WORK}/first" \
    --readiness "${WORK}/run/wall.json" \
    --mode live \
    --operators "${WORK}/operators.json" \
    --drill-seconds "$DRILL_SECONDS" >>"${WORK}/wall.log" 2>&1

if grep -q "sandbox ${VENUE}: balances" "${WORK}/first.log" && ! grep -q 'sandbox' "${WORK}/wall.log" \
   && grep -q 'backtest node' "${WORK}/run/first.json" 2>/dev/null; then
    say "PASS  the backtest matched on a simulated venue where the live run opened the venue"
else
    fail "the venue path was not the difference between the backtest and the live run"
fi

# Paper is wall-clocked, reads live inputs and reaches no venue.
CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --config "${WORK}/config.json" \
    --keystore "${WORK}/keystore.bin" \
    --credential drill-cred \
    --store "${WORK}/first" \
    --readiness "${WORK}/run/paper.json" \
    --mode paper \
    --drill-seconds "$DRILL_SECONDS" >>"${WORK}/paper.log" 2>&1

paper_stamp="$(grep -o '"stamped_at_ns": *"[0-9]*"' "${WORK}/run/paper.json" 2>/dev/null | grep -oE '[0-9]+' | tail -1)"

# Wall-clocked and named for its mode. One assertion cannot separate paper from both neighbours.
if [ -n "$paper_stamp" ] && [ "$paper_stamp" != "$REPLAY_AT" ] && [ "$paper_stamp" != "$LAST_AT" ] \
   && grep -q 'paper node' "${WORK}/run/paper.json" 2>/dev/null; then
    say "PASS  a paper run reads the wall clock, says so, and is neither of the other two modes"
else
    fail "the paper mode is not distinct from backtest"
    say  "      stamped ${paper_stamp:-<nothing>}"
fi

# A command line that does not name one valid run starts nothing. Resolving it either way would start a run that was not requested.
refused() { # <label> <arguments...> - passes when the engine exits non-zero and says it was the mode
    local label="$1" output code; shift
    output="$(CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
        --config "${WORK}/config.json" --keystore "${WORK}/keystore.bin" --credential drill-cred \
        --store "${WORK}/first" --readiness "${WORK}/run/refused.json" \
        "$@" --drill-seconds 1 2>&1)"
    code=$?
    if [ "$code" = 0 ]; then
        fail "${label} started anyway"
    elif ! printf '%s' "$output" | grep -qE -- '--mode|--replay-from'; then
        fail "${label} was refused for another reason: $(printf '%s' "$output" | head -1)"
    else
        say "PASS  ${label} is refused rather than resolved"
    fi
}
refused "a run that names no mode (the former default was live)"
refused "replay named as a mode" --mode replay --replay-from "$REPLAY_AT"
refused "a backtest with no recorded start" --mode backtest
refused "a backtest with no recorded market" --mode backtest --replay-from "$REPLAY_AT"
refused "a recorded start on a paper run" --mode paper --replay-from "$REPLAY_AT"
refused "a recorded start on a live run" --mode live --replay-from "$REPLAY_AT"
refused "a recorded market on a paper run" --mode paper --market-store "${WORK}/market" --market "$MARKET"

if [ "$status" = 0 ]; then
    say "PASS  the program's stream is a function of its clock, its record and its recording, and of nothing else"
fi
exit "$status"
