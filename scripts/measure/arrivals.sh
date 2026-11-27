#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Reads a venue's arrival rate off a market this node recorded.
# Usage: ./scripts/measure/arrivals.sh <store-root> <market-stream> [queue-bound]
# Exit codes: 0 measured, 1 it could not be.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

say() { printf '%s\n' "$*"; }

if [ "$#" -lt 2 ]; then
    say "usage: ${0##*/} <store-root> <market-stream> [queue-bound]"
    exit 1
fi

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH"; exit 1; }
if ! out="$(cd "${CORE}/crates/strategy_engine" && cargo build --quiet --offline --examples 2>&1)"; then
    say "FAIL  the examples did not build"
    printf '%s\n' "$out" | tail -3
    exit 1
fi

exec "${CORE}/target/debug/examples/measure_arrivals" "$@"
