#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# A first run on a live public market with no key and no hand-written profile, in four commands:
# draft the profile and a starter configuration from the venue's published rules, write a
# strategy, run it in paper on the live market, and backtest it over what the paper run recorded.
# Reaches a venue, so it is a drill an operator runs, not a gate. The venue and its endpoints are
# named by the operator (see docs/how_to/run_a_first_strategy_on_a_live_market.md). None is guessed.
# Needs a delegated cgroup v2 subtree. Exit codes: 0 both runs filled, 1 they did not, 2 not run.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

for name in BIJECTIVE_VENUE BIJECTIVE_INSTRUMENT BIJECTIVE_RULES_ENDPOINT BIJECTIVE_STREAM_ENDPOINT BIJECTIVE_CGROUP_ROOT; do
    if [ -z "${!name:-}" ]; then
        say "NOT VERIFIED  ${name} is not set; this drill names no venue and guesses no endpoint"
        exit 2
    fi
done
MINUTES="${BIJECTIVE_FIRST_RUN_MINUTES:-2}"

if ! out="$(cd "$CORE" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the workspace did not build, so the first run is UNVERIFIED"
    printf '%s\n' "$out" | tail -3
    exit 1
fi
export PATH="${CORE}/target/debug:${PATH}"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
cd "$WORK" || exit 1

if bijective-profile "$BIJECTIVE_VENUE" "$BIJECTIVE_INSTRUMENT" "$BIJECTIVE_RULES_ENDPOINT" --out setup >profile.log 2>&1; then
    say "PASS  1 the profile and a starter configuration were drafted from the venue's published rules"
else
    fail "1 the profile was not drafted: $(tail -1 profile.log)"
    exit 1
fi

if bijective-backtest --new first.py >new.log 2>&1; then
    say "PASS  2 a strategy that already runs was written"
else
    fail "2 no strategy was written: $(tail -1 new.log)"
    exit 1
fi

bijective-paper first.py setup "$BIJECTIVE_STREAM_ENDPOINT" --store store --minutes "$MINUTES" >paper.log 2>&1
paper_fills="$(grep -oE '[0-9]+ fill\(s\)' paper.log | grep -oE '^[0-9]+' | tail -1)"
if [ "${paper_fills:-0}" -ge 1 ] && grep -q '^refused:  none' paper.log; then
    say "PASS  3 the paper run filled ${paper_fills} order(s) on the simulated venue, none refused"
else
    fail "3 the paper run did not fill: $(grep -E 'fill|refused|bijective-paper' paper.log | tail -3)"
fi

bijective-backtest first.py --store store --market "market-${BIJECTIVE_VENUE}" --config setup/config.json >backtest.log 2>&1
backtest_fills="$(grep -oE '[0-9]+ fill\(s\)' backtest.log | grep -oE '^[0-9]+' | tail -1)"
if [ "${backtest_fills:-0}" -ge 1 ] && grep -q 'equity change' backtest.log; then
    say "PASS  4 the backtest over the recorded market filled ${backtest_fills} order(s) and stated its equity change"
else
    fail "4 the backtest did not fill: $(grep -E 'fill|refused|bijective-backtest' backtest.log | tail -3)"
fi

if [ "$status" -eq 0 ]; then
    say "PASS  a first strategy ran on a live market in paper and in backtest with no key and no hand-written profile"
fi
exit "$status"
