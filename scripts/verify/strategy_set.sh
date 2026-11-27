#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# A set of strategies over one window: every strategy is sent the same events, each run is bounded
# and recorded on its own, and a strategy that fails does not stop the others.
# Drives real runs, so it needs a cgroup v2 subtree: systemd-run --user --scope --setenv=BIJECTIVE_CGROUP_ROOT=<subtree> -- ./scripts/verify/strategy_set.sh
# Exit codes: 0 the set runs as described, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH, so a strategy set is UNVERIFIED"; exit 1; }
if ! out="$(cd "$CORE" && cargo build --quiet --offline -p bijective-strategy-engine 2>&1 \
        && cd "${CORE}/crates/serialization" && cargo build --quiet --offline --examples 2>&1)"; then
    say "FAIL  the programs did not build"
    printf '%s\n' "$out" | tail -3
    exit 1
fi
SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
SEED="${CORE}/target/debug/examples/seed_market"

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user"
    exit 1
fi
CGROUP="${CGROUP_ROOT}/bijective-set-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}"/* "${CGROUP}" 2>/dev/null || true' EXIT
printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null || {
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run" "${WORK}/set"
"$SEED" "${WORK}/store" "btc-perp" 6 50 >/dev/null 2>&1 || { say "FAIL  the market was not seeded"; exit 1; }

# Each strategy counts the events it is sent and proposes once at the end with the count, so the
# drill can see every run was sent the same window.
for name in alpha beta; do
    cat > "${WORK}/set/${name}.sh" <<STRATEGY
#!/bin/sh
seen=0
while IFS= read -r line; do
  case "\$line" in
    *end_of_window*) break ;;
    *'"event":"book"'*) seen=\$((seen + 1)) ;;
  esac
done
printf '{"emit":"proposal","desk":"${name}","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"${name}-%s","decided_at_ns":"1"}\n' "\$seen"
STRATEGY
done
printf '#!/bin/sh\nexit 3\n' > "${WORK}/set/broken.sh"
chmod +x "${WORK}/set/"*.sh

out="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/set/alpha.sh" --strategy "${WORK}/set/beta.sh" \
    --strategy "${WORK}/set/broken.sh" --market market-drill --to-position 3 --exchange whole 2>&1)"
code=$?

if [ "$code" -eq 0 ] && printf '%s' "$out" | grep -q 'alpha.sh: recorded at [0-9]*, 1 proposal(s) admitted' \
        && printf '%s' "$out" | grep -q 'beta.sh: recorded at [0-9]*, 1 proposal(s) admitted'; then
    say "PASS  two strategies in a set each ran, were recorded and had their proposal admitted"
else
    fail "the set did not run each strategy: exit ${code}; $(printf '%s' "$out" | tail -2)"
fi
if printf '%s' "$out" | grep -q 'broken.sh: the run ended'; then
    say "PASS  a strategy that failed is reported in its own line and stopped no other"
else
    fail "the failing strategy was not reported on its own: $(printf '%s' "$out" | tail -1)"
fi
refused="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/set/alpha.sh" --strategy "${WORK}/set/alpha.sh" \
    --market market-drill --to-position 3 --exchange whole 2>&1)"
if [ $? -eq 2 ] && printf '%s' "$refused" | grep -q 'given twice'; then
    say "PASS  the same strategy given twice is refused, not counted twice"
else
    fail "a duplicated strategy was not refused: ${refused}"
fi

# A paper session of two strategies: one read of the market, one writer of the proposal stream,
# each strategy answering every event it is sent.
for name in north south; do
    cat > "${WORK}/set/live-${name}.sh" <<STRATEGY
#!/bin/sh
n=0
while IFS= read -r line; do
  case "\$line" in
    *'"event":"book"'*)
      n=\$((n + 1))
      printf '{"emit":"proposal","desk":"${name}","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"${name}-%s","decided_at_ns":"1"}\n' "\$n"
      printf '{"emit":"answered"}\n'
      ;;
  esac
done
STRATEGY
done
chmod +x "${WORK}/set/"live-*.sh
"$SUPERVISOR" --readiness "${WORK}/run/live.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/live-north.sh" --strategy "${WORK}/set/live-south.sh" \
    --market market-drill --stage paper --drill-seconds 6 >"${WORK}/live.out" 2>&1 &
session=$!
sleep 2
"$SEED" "${WORK}/store" "btc-perp" 4 90 >/dev/null 2>&1
wait "$session"
live_out="$(cat "${WORK}/live.out")"
north="$(printf '%s' "$live_out" | grep -o 'live-north.sh: [^;]*; paper session resumed at [0-9]* after [^,]*, sent through [0-9]*, [1-9][0-9]* proposal(s) admitted')"
south="$(printf '%s' "$live_out" | grep -o 'live-south.sh: [^;]*; paper session resumed at [0-9]* after [^,]*, sent through [0-9]*, [1-9][0-9]* proposal(s) admitted')"
if [ -n "$north" ] && [ -n "$south" ]; then
    say "PASS  two strategies in one paper session were each sent the market and each had proposals admitted"
else
    fail "the paper session did not feed both strategies: $(tail -2 "${WORK}/live.out")"
fi

# One strategy reading two venues: both are sent, merged, each top naming its venue, and a
# decision names the venue its position is on.
"$SEED" "${WORK}/store" "btc-perp" 3 0 east >/dev/null 2>&1 || fail "the east market was not seeded"
"$SEED" "${WORK}/store" "btc-perp" 3 0 west >/dev/null 2>&1 || fail "the west market was not seeded"
cat > "${WORK}/set/spread.sh" <<'STRATEGY'
#!/bin/sh
east=0; west=0; sent=""
while IFS= read -r line; do
  case "$line" in
    *end_of_window*) break ;;
    *'"event":"book"'*'"venue":"east"'*) east=$((east + 1)); sent="${sent}e" ;;
    *'"event":"book"'*'"venue":"west"'*) west=$((west + 1)); sent="${sent}w" ;;
  esac
done
printf '{"emit":"proposal","desk":"arb","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"sent-%s","decided_at_ns":"1","venue":"east"}\n' "$sent"
printf '{"emit":"proposal","desk":"arb","instrument":"btc-perp","side":"sell","quantity_scaled":"1","price_scaled":"3000100","client_id":"w%se%s","decided_at_ns":"1","venue":"west"}\n' "$west" "$east"
printf '{"emit":"proposal","desk":"arb","instrument":"btc-perp","side":"sell","quantity_scaled":"1","price_scaled":"3000100","client_id":"unnamed","decided_at_ns":"1"}\n'
STRATEGY
chmod +x "${WORK}/set/spread.sh"
both="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/spread.sh" --market market-east --to-position 2 --market market-west --to-position 2 \
    --exchange whole 2>&1)"
runs="$(cd "${CORE}/crates/strategy_engine" \
    && cargo run --quiet --offline --example read_runs -- "${WORK}/store" 2>/dev/null)"
if printf '%s' "$both" | grep -q '6 event(s) sent to position 2 and 2 whole; recorded at [0-9]*, 2 proposal(s) admitted, 1 refused' \
    && printf '%s' "$both" | grep -q 'reads several venues, so a proposal names its venue' \
    && printf '%s' "$runs" | grep -q 'admitted [0-9]*\.\.=[0-9]* (1) on market-east through 2' \
    && printf '%s' "$runs" | grep -q 'admitted [0-9]*\.\.=[0-9]* (1) on market-west through 2'; then
    say "PASS  one strategy read two venues merged; each proposal is measured on the venue it names, one naming none refused"
else
    fail "a run over two venues did not send both or attribute by venue: ${both} ${runs}"
fi
# Both venues stamp and receive their tops at the same instants here, so arrival ties and the
# order the markets were named in decides: one of each in turn.
merged="$(cd "${CORE}/crates/strategy_engine" \
    && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" proposals-research 2>/dev/null)"
if printf '%s' "$merged" | grep -q 'sent-ewewew '; then
    say "PASS  two venues were sent in arrival order, ties in the order the markets were named"
else
    fail "two venues were not merged in arrival order: ${merged}"
fi
# Stepwise over two venues: a decision names the venue and a position on it that was sent. One
# naming a venue the run was not sent is refused.
cat > "${WORK}/set/named.sh" <<'STRATEGY'
#!/bin/sh
at=""
while IFS= read -r line; do
  case "$line" in
    *'"event":"book"'*'"venue":"east"'*)
      at=$(printf '%s' "$line" | sed 's/.*"position":"\([0-9]*\)".*/\1/')
      printf '{"emit":"answered"}\n' ;;
    *'"event":"book"'*) printf '{"emit":"answered"}\n' ;;
    *end_of_window*) break ;;
  esac
done
printf '{"emit":"proposal","desk":"arb","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"east-at","decided_at_ns":"1","venue":"east","decided_at_position":"%s"}\n' "$at"
printf '{"emit":"proposal","desk":"arb","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"north-at","decided_at_ns":"1","venue":"north","decided_at_position":"%s"}\n' "$at"
STRATEGY
chmod +x "${WORK}/set/named.sh"
stepped="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/named.sh" --market market-east --to-position 2 --market market-west --to-position 2 \
    --exchange stepwise 2>&1)"
if printf '%s' "$stepped" | grep -q '1 proposal(s) admitted, 1 refused' \
    && printf '%s' "$stepped" | grep -q 'the proposal names north, which this run was not sent'; then
    say "PASS  stepwise over two venues: a decision on a sent venue was placed, one on another refused"
else
    fail "a stepwise run over two venues did not place by venue: ${stepped}"
fi
# A paper session over two venues: each resumes at its own present, every top names its venue,
# and each proposal is measured on the market of the venue it names.
cat > "${WORK}/set/pair.sh" <<'STRATEGY'
#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"event":"book"'*'"venue":"east"'*) venue=east ;;
    *'"event":"book"'*'"venue":"west"'*) venue=west ;;
    *) continue ;;
  esac
  printf '{"emit":"proposal","desk":"arb","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"%s","decided_at_ns":"1","venue":"%s"}\n' "$venue" "$venue"
  printf '{"emit":"answered"}\n'
done
STRATEGY
chmod +x "${WORK}/set/pair.sh"
"$SUPERVISOR" --readiness "${WORK}/run/pair.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/pair.sh" --market market-east --market market-west \
    --stage paper --drill-seconds 6 >"${WORK}/pair.out" 2>&1 &
session=$!
sleep 2
"$SEED" "${WORK}/store" "btc-perp" 2 0 east >/dev/null 2>&1
"$SEED" "${WORK}/store" "btc-perp" 2 0 west >/dev/null 2>&1
wait "$session"
pair="$(cat "${WORK}/pair.out")"
paired="$(cd "${CORE}/crates/strategy_engine" \
    && cargo run --quiet --offline --example read_runs -- "${WORK}/store" 2>/dev/null)"
if printf '%s' "$pair" | grep -q 'paper session resumed at 3 and 3 after a gap nothing records and a gap nothing records, sent through 4 and 4, 4 proposal(s) admitted' \
    && printf '%s' "$paired" | grep -q 'on market-east through 4' \
    && printf '%s' "$paired" | grep -q 'on market-west through 4'; then
    say "PASS  a paper session read two venues, each from its own present, measured on the venue named"
else
    fail "a paper session over two venues did not hold: ${pair} ${paired}"
fi

# A venue's facts beside its tops: each fact goes before the top that arrived with it, a decision
# is still placed on the market stream, and a run of facts alone is refused.
SEED_FACTS="${CORE}/target/debug/examples/seed_facts"
"$SEED" "${WORK}/store" "btc-perp" 3 0 north >/dev/null 2>&1 || fail "the north market was not seeded"
"$SEED_FACTS" "${WORK}/store" "btc-perp" 3 north >/dev/null 2>&1 || fail "the north facts were not seeded"
cat > "${WORK}/set/facts.sh" <<'STRATEGY'
#!/bin/sh
sent=""
while IFS= read -r line; do
  case "$line" in
    *end_of_window*) break ;;
    *'"event":"fact"'*'"kind":"mark_price"'*) sent="${sent}f" ;;
    *'"event":"book"'*) sent="${sent}b" ;;
  esac
done
printf '{"emit":"proposal","desk":"basis","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"facts-%s","decided_at_ns":"1"}\n' "$sent"
STRATEGY
chmod +x "${WORK}/set/facts.sh"
with_facts="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/facts.sh" --market market-north --to-position 2 --market facts-north --to-position 2 \
    --exchange whole 2>&1)"
told="$(cd "${CORE}/crates/strategy_engine" \
    && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" proposals-research 2>/dev/null)"
if printf '%s' "$with_facts" | grep -q '1 proposal(s) admitted, 0 refused' \
    && printf '%s' "$told" | grep -q 'facts-fbfbfb '; then
    say "PASS  a venue's facts were sent beside its tops, each fact before the top it arrived with"
else
    fail "facts and tops were not sent together in arrival order: ${with_facts} ${told}"
fi
facts_only="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/facts.sh" --market facts-north --to-position 2 --exchange whole 2>&1)"
if printf '%s' "$facts_only" | grep -q 'reads at least one market stream'; then
    say "PASS  a run of facts alone is refused: decisions are placed on a market's tops"
else
    fail "a run of facts alone was accepted: ${facts_only}"
fi

mismatched="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" --store "${WORK}/store" \
    --strategy "${WORK}/set/spread.sh" --market market-east --market market-west --to-position 2 \
    --exchange whole 2>&1)"
if printf '%s' "$mismatched" | grep -q 'each market needs its own end'; then
    say "PASS  a market without its own end is refused"
else
    fail "two markets with one end were accepted: ${mismatched}"
fi

if [ "$status" -eq 0 ]; then
    say "PASS  one window, many strategies, each bounded and recorded on its own"
fi
exit "$status"
