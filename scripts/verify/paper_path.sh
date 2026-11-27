#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# A strategy's order filled end to end, with nothing but the platform's own programs and the store:
# a paper strategy session proposes, an operator resumes the node with a signed command, the Control
# Engine in paper mode admits the proposal against its limits, the simulated venue fills it against
# the market the node reads, reconciliation records the fill, the strategy is told it filled, and the
# desk's and the strategy's exposure are judged from it.
# Needs a delegated cgroup v2 subtree (see scripts/verify/sandbox.sh). Exit codes: 0 the order
# filled and every step is on record, 1 it did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the paper path is UNVERIFIED"
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
        say "FAIL  ${target} did not build, so the paper path is UNVERIFIED"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
for target in crates/serialization crates/control_engine crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target}'s examples did not build, so the path cannot be seeded or read"
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
    say "              this drill will not guess one, and the paper path is unproven until it runs"
    exit 1
fi
CGROUP="${CGROUP_ROOT}/bijective-paper-$$"
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
# Room for one order of a few dozen in money, and a desk and a strategy limit, by the name the
# supervisor runs the strategy under (its file's stem), that the fill must be judged against.
sed -i 's|"maximum_per_order_scaled": "100000000"|"maximum_per_order_scaled": "100000000000"|' "${WORK}/config.json"
sed -i 's|^    { "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "100000000000" }$|&,\n    { "scope": "desk:alpha", "kind": "gross_exposure", "maximum_money_scaled": "50000000000" },\n    { "scope": "strategy:strategy", "kind": "gross_exposure", "maximum_money_scaled": "50000000000" }|' "${WORK}/config.json"
grep -q '"strategy:strategy"' "${WORK}/config.json" || fail "the drill could not declare its desk and strategy limits"

# RFC 8032's first test key: the drill registry holds its public half, so this signs as the
# drill operator. Public by design. It protects nothing outside a drill.
printf '%s\n' 9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60 > "${WORK}/operator.key"

# Buys once, above the ask, on its first book. Reports each answer it is told about that order as
# a further proposal named for it, so the strategy's own output witnesses what reached it.
cat > "${WORK}/strategy.sh" <<STRATEGY
#!/bin/sh
proposed=0; filled=0
while IFS= read -r line; do
  case "\$line" in
    *'"event":"book"'*)
      if [ "\$proposed" -eq 0 ]; then
        proposed=1
        printf '{"emit":"proposal","desk":"alpha","instrument":"${INSTRUMENT}","side":"buy","quantity_scaled":"1","price_scaled":"2660000","client_id":"paper-buy","decided_at_ns":"1"}\n'
      fi ;;
    *'"event":"order"'*'"status":"filled"'*)
      if [ "\$filled" -eq 0 ]; then
        filled=1
        printf '{"emit":"proposal","desk":"alpha","instrument":"${INSTRUMENT}","side":"sell","quantity_scaled":"1","price_scaled":"2600000","client_id":"heard-filled","decided_at_ns":"2"}\n'
      fi ;;
  esac
done
STRATEGY
chmod +x "${WORK}/strategy.sh"

# The market keeps arriving, stamped now, so the node's marks stay within its freshness bound.
now_ms() { date +%s%3N; }
"$SEED" "${WORK}/store" "$INSTRUMENT" 1 0 "$VENUE" "$(now_ms)" >/dev/null 2>&1 || fail "the market was not seeded"
( while :; do "$SEED" "${WORK}/store" "$INSTRUMENT" 1 0 "$VENUE" "$(now_ms)" >/dev/null 2>&1; sleep 1; done ) &
seeder=$!

"$SUPERVISOR" --readiness "${WORK}/run/strategy-engine.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/strategy.sh" --market "market-${VENUE}" \
    --stage paper --drill-seconds 34 >"${WORK}/strategy-engine.log" 2>&1 &
session=$!
sleep 2

# Halted until an operator resumes it, as every node starts. The signed command is the only way.
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
    --drill-seconds 25 >"${WORK}/control-engine.log" 2>&1
wait "$session"
kill "$seeder" 2>/dev/null; seeder=""

control="$(cd "${CORE}/crates/control_engine" \
    && cargo run --quiet --offline --example read_control_stream -- "${WORK}/store" control-paper 2>/dev/null)"
told="$(cd "${CORE}/crates/serialization" \
    && cargo run --quiet --offline --example read_feedback -- "${WORK}/store" control-paper 2>/dev/null)"
asked="$(cd "${CORE}/crates/strategy_engine" \
    && cargo run --quiet --offline --example read_proposals -- "${WORK}/store" proposals-paper 2>/dev/null)"

show_logs() {
    printf '%s
' "$control" | grep -E 'Refused|SafeState' | head -3 | sed 's/^/      record: /'
    tail -6 "${WORK}/control-engine.log" | sed 's/^/      control: /'
    tail -3 "${WORK}/strategy-engine.log" | sed 's/^/      strategy: /'
}

if grep -q "sandbox ${VENUE}: balances" "${WORK}/control-engine.log"; then
    say "PASS  the paper node opened its sandbox account and stated it"
else
    fail "the paper node did not state its sandbox account"
fi
if printf '%s' "$control" | grep -q 'CommandApplied .*action: Resume.*state: Active'; then
    say "PASS  the signed resume was carried out and read back active"
else
    fail "the node was not resumed by the signed command"
    show_logs
fi
if printf '%s' "$control" | grep -q 'Intent .*owner: Some(Owner { desk: "alpha", strategy: Some("strategy") })'; then
    say "PASS  the order was placed for the desk and the strategy that proposed it"
else
    fail "no intent names the desk and strategy that proposed it"
    show_logs
fi
# The venue is sent the identifier the node assigned. The strategy's own is the decision's.
order_id="$(printf '%s' "$control" | sed -n 's/.*Intent { decision_id: "paper-buy".*client_id: "\(bl-[a-z0-9-]*\)".*/\1/p' | head -1)"
if [ -n "$order_id" ] && printf '%s' "$control" | grep -q "Filled { client_id: \"${order_id}\""; then
    say "PASS  the simulated venue filled the order under the identifier the node assigned"
else
    fail "no fill of the proposed order, under a node-assigned identifier, is on the control stream"
    show_logs
fi
if printf '%s' "$told" | grep -q '^proposal 0 filled'; then
    say "PASS  the fill was mirrored to the strategy's feedback"
else
    fail "the feedback does not tell proposal 0 it filled: ${told}"
fi
if printf '%s' "$asked" | grep -q 'heard-filled'; then
    say "PASS  the strategy that proposed was told its order filled, and acted on it"
else
    fail "the strategy did not report hearing its fill"
    show_logs
fi
if printf '%s' "$control" | grep -q 'ScopesValued .*Desk("alpha").*Strategy("strategy")'; then
    say "PASS  the desk's and the strategy's figures were recorded for the console"
else
    fail "no scoped figures name the desk and the strategy"
fi
if grep -q 'unmeasured\|Unjudgeable' "${WORK}/control-engine.log" \
    || printf '%s' "$control" | grep -q 'Unjudgeable'; then
    fail "a desk or strategy figure was unmeasured after the fill"
    show_logs
else
    say "PASS  the desk's and the strategy's exposure were judged with the position held"
fi

live="$(cd "${CORE}/crates/control_engine"     && cargo run --quiet --offline --example read_control_stream -- "${WORK}/store" control 2>/dev/null)"
if printf '%s' "$live" | grep -q 'Intent\|Filled'; then
    fail "the paper run wrote to the live node's record"
else
    say "PASS  nothing of the paper run reached the live node's record"
fi

if [ "$status" -eq 0 ]; then
    say "PASS  a strategy's order fills end to end and every step is on record"
fi
exit "$status"
