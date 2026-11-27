#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Verifies a node's record carried to another host reads the same and carries no credential (runbook 07).
# Same strategies, byte-identical page, strategy found by name after its file moved, same stage sequence, no secret in the store.
# Needs a delegated cgroup subtree to record real runs. Exit codes: 0 the record survives the move, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH"; exit 1; }
if [ -z "${BIJECTIVE_CGROUP_ROOT:-}" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a delegated cgroup v2 subtree; a migration"
    say "              carries the record of real runs, and this will not invent them"
    exit 1
fi

for target in crates/strategy_engine crates/evaluation; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build"; printf '%s\n' "$out" | tail -3; exit 1
    fi
done

BACKTEST="${CORE}/target/debug/bijective-backtest"
REPORT="${CORE}/target/debug/bijective-report"
PROMOTE="${CORE}/target/debug/bijective-promote"

origin="${WORK}/origin"
mkdir -p "$origin"
(cd "$origin" && "$BACKTEST" --new carried.sh >/dev/null 2>&1)
(cd "$origin" && "$BACKTEST" carried.sh >/dev/null 2>&1)
(cd "$origin" && "$BACKTEST" carried.sh >/dev/null 2>&1)

before_list="$("$REPORT" "${origin}/store" 2>&1)"
before_page="$("$REPORT" "${origin}/store" carried 19600 2>&1)"
before_stages="$("$PROMOTE" "${origin}/store" carried 19600 2>&1)"

if printf '%s' "$before_list" | grep -q 'carried'; then
    say "PASS  the origin holds a strategy worth carrying"
else
    fail "nothing was recorded at the origin: ${before_list}"
    exit 1
fi

# Copy the store and nothing else, to a new directory with the strategy at a new path. Copied so a failure leaves both sides.
destination="${WORK}/destination"
mkdir -p "${destination}/somewhere/else"
cp -r "${origin}/store" "${destination}/store"
cp "${origin}/carried.sh" "${destination}/somewhere/else/differently-named.sh"

after_list="$("$REPORT" "${destination}/store" 2>&1)"
after_page="$("$REPORT" "${destination}/store" carried 19600 2>&1)"
after_stages="$("$PROMOTE" "${destination}/store" carried 19600 2>&1)"

# The store's path is in the heading, so only that line is expected to differ.
if diff <(printf '%s' "$before_list" | grep -v '^# Strategies in') \
        <(printf '%s' "$after_list" | grep -v '^# Strategies in') >/dev/null; then
    say "PASS  the record says the same thing after the move"
else
    fail "the listing changed on the way"
    diff <(printf '%s' "$before_list") <(printf '%s' "$after_list") | head -6 | sed 's/^/      /'
fi

if diff <(printf '%s' "$before_page" | grep -v '^Strategy ') \
        <(printf '%s' "$after_page" | grep -v '^Strategy ') >/dev/null; then
    say "PASS  the page is the same page, and the strategy was found by name after its file moved"
else
    fail "the page differs after the move"
    diff <(printf '%s' "$before_page") <(printf '%s' "$after_page") | head -8 | sed 's/^/      /'
fi

if [ "$(printf '%s' "$before_stages" | tail -n +2)" = "$(printf '%s' "$after_stages" | tail -n +2)" ]; then
    say "PASS  the promotion stages answers identically on the other side"
else
    fail "the promotion stages changed its answer on a copy"
    diff <(printf '%s' "$before_stages") <(printf '%s' "$after_stages") | head -6 | sed 's/^/      /'
fi

# A keystore in a store would travel with every backup and migration, so look for it.
secrets="$(find "${destination}/store" -type f \( -name '*.bjks' -o -name '*key*' -o -name '*secret*' -o -name '*credential*' \) 2>/dev/null | head -3)"
if [ -z "$secrets" ]; then
    say "PASS  the store carried no credential, so a move cannot take a key to a new host"
else
    fail "something credential-shaped travelled with the record:"
    printf '%s\n' "$secrets" | sed 's/^/      /'
fi

if [ "$status" = 0 ]; then
    say "PASS  a node's record survives a move, and carries no secret with it"
fi
exit "$status"
