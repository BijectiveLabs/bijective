#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# A paper node whose market has gone quiet refuses a burst of proposals and stays active, then
# trades again once the market is current. The market is seeded once, stamped a minute in the past,
# so every mark the node holds is older than its freshness bound. The strategy proposes a burst on
# that book: each proposal must be refused as stale, none may reach the simulated venue, and the
# node must not be driven to a safe state, since stale figures stop the order and not the platform.
# Then the market resumes, stamped now, and the strategy's next proposal must be placed and filled.
# Needs a delegated cgroup v2 subtree (see scripts/verify/sandbox.sh). Exit codes: 0 every refusal
# and the recovery are on record, 1 they are not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

BURST=20
status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the stale market drill is UNVERIFIED"
    exit 1
fi

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
if [ -z "$VENUE" ]; then
    say "FAIL  the build registers no venue, so no configuration can name one"
    exit 1
fi

# Built here, not assumed: a stale binary proves the path for a build that is not shipped.
for target in crates/control_engine crates/strategy_engine crates/cli; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build, so the stale market drill is UNVERIFIED"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
for target in crates/serialization crates/control_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build, so the market cannot be seeded or read"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"
SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
OPERATOR="${CORE}/target/debug/bijective"
SEED="${CORE}/target/debug/examples/seed_market"
for program in "$CONTROL_ENGINE" "$SUPERVISOR" "$OPERATOR" "$SEED"; do
    [ -x "$program" ] || { say "FAIL  ${program} is missing after a successful build"; exit 1; }
done

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the stale market refusal is unproven"
    exit 1
fi
CGROUP="${CGROUP_ROOT}/bijective-stale-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
seeder=""
trap 'rm -rf "${WORK}"; [ -n "$seeder" ] && kill "$seeder" 2>/dev/null; rmdir "${CGROUP}" 2>/dev/null || true' EXIT
if ! printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null; then
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"
    exit 1
fi

mkdir -p "${WORK}/store" "${WORK}/run"
# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "BTCUSDT"
INSTRUMENT="${VENUE}.BTCUSDT"
sed -i 's|"maximum_per_order_scaled": "100000000"|"maximum_per_order_scaled": "100000000000"|' "${WORK}/config.json"
grep -q '"freshness_bound_ns": "5000000000"' "${WORK}/config.json" \
    || fail "the fixture no longer bounds freshness at five seconds, which this drill relies on"

# RFC 8032's first test key: the drill registry holds its public half. It protects nothing
# outside a drill.
printf '%s\n' 9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60 > "${WORK}/operator.key"

# On its first book, which is stale, a burst of proposals, limit and market orders in turn: a
# market order carries no price for the collar to judge. On its second, which is the market
# resuming, one more. Each answer it is told is reported back as a proposal named for it.
cat > "${WORK}/strategy.sh" <<STRATEGY
#!/bin/sh
books=0; filled=0
while IFS= read -r line; do
  case "\$line" in
    *'"event":"book"'*)
      books=\$((books + 1))
      if [ "\$books" -eq 1 ]; then
        i=0
        while [ "\$i" -lt ${BURST} ]; do
          if [ \$((i % 2)) -eq 0 ]; then
            printf '{"emit":"proposal","desk":"alpha","instrument":"${INSTRUMENT}","side":"buy","quantity_scaled":"1","price_scaled":"2660000","client_id":"stale-%s","decided_at_ns":"%s"}\n' "\$i" "\$((i + 1))"
          else
            printf '{"emit":"proposal","desk":"alpha","instrument":"${INSTRUMENT}","side":"buy","quantity_scaled":"1","cap_scaled":"2660000","client_id":"stale-%s","decided_at_ns":"%s"}\n' "\$i" "\$((i + 1))"
          fi
          i=\$((i + 1))
        done
      elif [ "\$books" -eq 2 ]; then
        printf '{"emit":"proposal","desk":"alpha","instrument":"${INSTRUMENT}","side":"buy","quantity_scaled":"1","price_scaled":"2660000","client_id":"fresh-buy","decided_at_ns":"100"}\n'
      fi ;;
    *'"event":"order"'*'"status":"filled"'*)
      if [ "\$filled" -eq 0 ]; then
        filled=1
        printf '{"emit":"proposal","desk":"alpha","instrument":"${INSTRUMENT}","side":"sell","quantity_scaled":"1","price_scaled":"2600000","client_id":"heard-filled","decided_at_ns":"101"}\n'
      fi ;;
  esac
done
STRATEGY
chmod +x "${WORK}/strategy.sh"

now_ms() { date +%s%3N; }

"$SUPERVISOR" --readiness "${WORK}/run/strategy-engine.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/strategy.sh" --market "market-${VENUE}" \
    --stage paper --drill-seconds 34 >"${WORK}/strategy-engine.log" 2>&1 &
session=$!
sleep 2
# Seeded once the session is live, which resumes at the present of the stream and so would never
# send a book written before it. A minute old: twelve times the bound, so no delay makes it fresh.
"$SEED" "${WORK}/store" "$INSTRUMENT" 1 0 "$VENUE" "$(( $(now_ms) - 60000 ))" >/dev/null 2>&1 \
    || fail "the stale market was not seeded"

if ! "$OPERATOR" issue --paper "${WORK}/store" drill-operator operator "${WORK}/operator.key" resume \
        >"${WORK}/issue.log" 2>&1; then
    fail "the resume command was not issued: $(tail -1 "${WORK}/issue.log")"
fi

"$CONTROL_ENGINE" \
    --mode paper \
    --operators "${WORK}/operators.json" \
    --config "${WORK}/config.json" \
    --store "${WORK}/store" \
    --readiness "${WORK}/run/control-engine.json" \
    --drill-seconds 25 >"${WORK}/control-engine.log" 2>&1 &
control_engine=$!

READ_CONTROL="${CORE}/target/debug/examples/read_control_stream"
[ -x "$READ_CONTROL" ] || { say "FAIL  ${READ_CONTROL} is missing after a successful build"; exit 1; }
# The market resumes only once every proposal of the burst has been judged against the stale book.
judged=0
for _ in $(seq 1 40); do
    judged="$("$READ_CONTROL" "${WORK}/store" control-paper 2>/dev/null | grep -c "decision_id: \"stale-")"
    [ "$judged" -ge "$BURST" ] && break
    sleep 0.5
done
[ "$judged" -ge "$BURST" ] || fail "only ${judged} of ${BURST} proposals were judged within 20 seconds"
( while :; do "$SEED" "${WORK}/store" "$INSTRUMENT" 1 0 "$VENUE" "$(now_ms)" >/dev/null 2>&1; sleep 1; done ) &
seeder=$!

wait "$control_engine"
wait "$session"
kill "$seeder" 2>/dev/null; seeder=""

control="$(cd "${CORE}/crates/control_engine" \
    && cargo run --quiet --offline --example read_control_stream -- "${WORK}/store" control-paper 2>/dev/null)"

show_logs() {
    printf '%s\n' "$control" | grep -E 'Refused|SafeState' | head -3 | sed 's/^/      record: /'
    tail -6 "${WORK}/control-engine.log" | sed 's/^/      control: /'
    tail -3 "${WORK}/strategy-engine.log" | sed 's/^/      strategy: /'
}

if printf '%s' "$control" | grep -q 'CommandApplied .*action: Resume.*state: Active'; then
    say "PASS  the signed resume was carried out and read back active"
else
    fail "the node was not resumed by the signed command"
    show_logs
fi

# Either guard is the platform's: the pre-trade check on stale figures, or the collar on a mark
# older than the same bound. Any other refusal is not this drill's, and fails it.
refused="$(printf '%s\n' "$control" | grep -E 'Refused \{ decision_id: "stale-[0-9]+"' \
    | grep -cE 'Stale \{|ns old, beyond the [0-9]+ ns bound')"
if [ "$refused" -eq "$BURST" ]; then
    say "PASS  all ${BURST} proposals made on the stale market were refused for its age"
else
    fail "${refused} of ${BURST} proposals made on the stale market were refused for its age"
    printf '%s\n' "$control" | grep 'stale-' | head -3 | sed 's/^/      record: /'
    show_logs
fi
if printf '%s' "$control" | grep -q 'Intent { decision_id: "stale-'; then
    fail "a proposal made on the stale market was placed"
    show_logs
else
    say "PASS  no proposal made on the stale market reached the venue"
fi
if printf '%s' "$control" | grep -q 'SafeStateDriven'; then
    fail "stale figures drove the node to a safe state, when they must stop only the order"
    show_logs
else
    say "PASS  the node stayed active through the burst: the orders stopped, the platform did not"
fi

order_id="$(printf '%s' "$control" | sed -n 's/.*Intent { decision_id: "fresh-buy".*client_id: "\(bl-[a-z0-9-]*\)".*/\1/p' | head -1)"
if [ -n "$order_id" ] && printf '%s' "$control" | grep -q "Filled { client_id: \"${order_id}\""; then
    say "PASS  once the market was current, the next proposal was placed and filled"
else
    fail "the proposal made after the market resumed was not placed and filled"
    show_logs
fi

if [ "$status" -eq 0 ]; then
    say "PASS  a stale market refuses a burst of orders and the node trades again once it is current"
fi
exit "$status"
