#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Market history from a client's file becomes the market stream a recording writes (decision 0016):
# a CSV is imported, imported again into a fresh store to the same bytes, a JSON Lines copy reads
# the same, a value finer than the instrument is refused with its line and nothing is written,
# a store's market is never added to, and a backtest fills on the imported market.
# Needs no venue and no cgroup. Exit codes: 0 every check passed, 1 one did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH, so the import path is UNVERIFIED"; exit 1; }

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
[ -n "$VENUE" ] || { say "FAIL  the build registers no venue, so no profile can name one"; exit 1; }

for target in crates/data_engine crates/control_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build, so the import path is UNVERIFIED"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
for target in crates/venue crates/serialization crates/control_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

IMPORT="${CORE}/target/debug/bijective-import"
CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
drill_profile "$CORE" "$VENUE" "BTCUSDT" "${WORK}/profile.json"
drill_config "$VENUE" "${WORK}/profile.json" "${VENUE}.BTCUSDT" "${WORK}/config.json"
sed -i 's|"maximum_per_order_scaled": "100000000"|"maximum_per_order_scaled": "100000000000"|' "${WORK}/config.json"

FIRST_MS=1800000000000
{
    echo "ts,symbol,bid,bid_size,ask,ask_size"
    for i in $(seq 0 29); do
        echo "$(( FIRST_MS + i * 100 )),BTCUSDT,26543.$(( 10 + i % 9 ))0,1.5,26545.00,0.9"
    done
} > "${WORK}/history.csv"
COLUMNS="time=ts,instrument=symbol,bid_price=bid,bid_quantity=bid_size,ask_price=ask,ask_quantity=ask_size"

import_into() { # <store> <file>
    "$IMPORT" "$2" --profile "${WORK}/profile.json" --store "$1" --time-unit ms --columns "$COLUMNS" 2>&1
}
market_bytes() { # <store>
    find "$1" -type f -path "*market-${VENUE}*" ! -name '*.lock' -exec sha256sum {} + | awk '{print $1}' | sort | paste -sd, -
}

out="$(import_into "${WORK}/first" "${WORK}/history.csv")"
if printf '%s' "$out" | grep -q "imported 30 book(s) into market-${VENUE}"; then
    say "PASS  the file was imported as the venue's market stream ($out)"
else
    fail "the file was not imported: $out"
fi

import_into "${WORK}/second" "${WORK}/history.csv" >/dev/null
if [ -n "$(market_bytes "${WORK}/first")" ] && [ "$(market_bytes "${WORK}/first")" = "$(market_bytes "${WORK}/second")" ]; then
    say "PASS  importing the same file into a fresh store wrote the same bytes"
else
    fail "two imports of one file differ"
fi

tail -n +2 "${WORK}/history.csv" | awk -F, '{printf "{\"ts\":%s,\"symbol\":\"%s\",\"bid\":\"%s\",\"bid_size\":\"%s\",\"ask\":\"%s\",\"ask_size\":\"%s\"}\n",$1,$2,$3,$4,$5,$6}' > "${WORK}/history.jsonl"
import_into "${WORK}/json" "${WORK}/history.jsonl" >/dev/null
if [ "$(market_bytes "${WORK}/first")" = "$(market_bytes "${WORK}/json")" ]; then
    say "PASS  a JSON Lines copy of the file reads to the same bytes"
else
    fail "the JSON Lines copy reads differently from the CSV"
fi

sed '5s/26545.00/26545.005/' "${WORK}/history.csv" > "${WORK}/fine.csv"
out="$(import_into "${WORK}/fine" "${WORK}/fine.csv")"
if printf '%s' "$out" | grep -q 'line 5: ask_price' && [ -z "$(market_bytes "${WORK}/fine" 2>/dev/null)" ]; then
    say "PASS  a value finer than the instrument is refused with its line and nothing is written"
else
    fail "a value finer than the instrument was not refused by line: $out"
fi

out="$(import_into "${WORK}/first" "${WORK}/history.csv")"
if printf '%s' "$out" | grep -q 'never added to'; then
    say "PASS  a store's market is never added to"
else
    fail "an import into a store that holds the market was not refused: $out"
fi

mkdir -p "${WORK}/record" "${WORK}/run"
"${CORE}/target/debug/examples/seed_proposals" "${WORK}/record" "${VENUE}.BTCUSDT" --backtest >/dev/null
"$CONTROL_ENGINE" --mode backtest --replay-from "$(( FIRST_MS * 1000000 ))" --config "${WORK}/config.json" \
    --store "${WORK}/record" --market-store "${WORK}/first" --market "market-${VENUE}" \
    --readiness "${WORK}/run/backtest.json" >"${WORK}/backtest.log" 2>&1
if grep -q "^${VENUE}: 2 order(s) at the venue, 2 fill(s)" "${WORK}/backtest.log"; then
    say "PASS  a backtest over the imported market filled its orders"
else
    fail "the backtest over the imported market did not fill: $(tail -3 "${WORK}/backtest.log")"
fi

if [ "$status" -eq 0 ]; then
    say "PASS  market history from a file is a recording the platform reads as its own"
fi
exit "$status"
