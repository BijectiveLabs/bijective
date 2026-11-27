#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# An interrupted start leaves the platform halted with no step responsible.
# The start is interrupted during the Control Engine's replay: the program runs but its limit state is unverified.
# Exit codes: 0 the interruption left the node safe and said so, 1 it did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KIT="${CORE}/crates/deployment"
MANIFEST="${KIT}/deployment.json"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH; the drill is UNVERIFIED"; exit 1; }

for crate in crates/data_engine crates/control_engine crates/deployment; do
    if ! out="$(cd "${CORE}/${crate}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${crate} did not build, so the drill is UNVERIFIED"
        printf '%s\n' "$out" | sed 's/^/        /' | head -5
        exit 1
    fi
done

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | head -1)"
FIXTURE="$(find "${CORE}/crates/adapters/${VENUE}/fixtures" -name '*.jsonl' 2>/dev/null | sort | head -1)"
[ -n "$FIXTURE" ] || { say "FAIL  no recorded traffic for the registered venue"; exit 1; }

DATA_ENGINE="${CORE}/target/debug/bijective-data-engine"
CONTROL_ENGINE="${CORE}/target/debug/bijective-control-engine"
DEPLOY="${CORE}/target/debug/bijective-deploy"

mkdir -p "${WORK}/store" "${WORK}/run"

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
SYMBOLS="$(grep -o '\\"s\\":\\"[A-Z0-9]*\\"' "$FIXTURE" | sed 's/.*:\\"//; s/\\"//' | sort -u | paste -sd, -)"
[ -n "$SYMBOLS" ] || { say "FAIL  the recording names no instrument, so no profile can declare one"; exit 1; }
drill_node_fixtures "$CORE" "$WORK" "$VENUE" "$SYMBOLS"

node_state() {
    "$DEPLOY" status "$MANIFEST" "${WORK}/run" 2>/dev/null | head -1 | awk '{print $1}'
}

[ "$(node_state)" = "Down" ] \
    || fail "an unstarted node reports $(node_state), not Down"

"$DATA_ENGINE" --store "${WORK}/store" --readiness "${WORK}/run/data-engine.json" \
    --venue "$VENUE" --profile "${WORK}/profile.json" \
    --recording "$FIXTURE" --drill-seconds 25 >"${WORK}/de.log" 2>&1 &
data_pid=$!

CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --mode live \
    --operators "${WORK}/operators.json" \
    --config "${WORK}/config.json" --keystore "${WORK}/keystore.bin" --credential drill-cred \
    --store "${WORK}/store" --readiness "${WORK}/run/control-engine.json" \
    --drill-seconds 25 >"${WORK}/ce.log" 2>&1 &
control_pid=$!

waited=0
while [ "$waited" -lt 40 ] && [ "$(node_state)" != "Ready" ]; do
    sleep 1
    waited=$((waited + 1))
done
[ "$(node_state)" = "Ready" ] || fail "the node never came up, so there was nothing to interrupt"

# SIGKILL, not a clean stop: the platform must get no chance to run code on the way out.
kill -9 "$control_pid" 2>/dev/null
wait "$control_pid" 2>/dev/null

# The last Ready claim stays on disk and reads Ready until stale. Correct, since readiness is a claim with a lifetime.
sleep 17

state="$(node_state)"
if [ "$state" = "Failed" ]; then
    say "PASS  the killed engine is reported as stopped, not as running"
else
    fail "a killed Control Engine leaves the node reporting ${state}"
fi

# Captured, not piped to grep: status exits non-zero when not ready and pipefail would fail the check on a correct node.
report="$("$DEPLOY" status "$MANIFEST" "${WORK}/run" 2>/dev/null)"
case "$report" in
    *"stopped reporting"*) say "PASS  the node names which program stopped and why" ;;
    *) fail "the node does not say the program stopped reporting: ${report%%$'\n'*}" ;;
esac

# No halt step exists to check. The check is that no program ever reported itself ready to trade.
if grep -rqi 'trading halted' "${WORK}/run/control-engine.json" 2>/dev/null; then
    say "PASS  the interrupted engine's last word was that trading was halted"
else
    fail "the Control Engine never recorded the trading state"
fi

# A stale writer lock is never broken automatically, so a restart after an interruption must refuse, not replay.
CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --mode live \
    --operators "${WORK}/operators.json" \
    --config "${WORK}/config.json" --keystore "${WORK}/keystore.bin" --credential drill-cred \
    --store "${WORK}/store" --readiness "${WORK}/run/control-engine.json" \
    --drill-seconds 0 >"${WORK}/ce2.log" 2>&1
held_exit=$?

if [ "$held_exit" != 0 ] && grep -q 'already held by a writer' "${WORK}/ce2.log"; then
    say "PROVEN a restart refuses the interrupted run's writer rather than racing it"
else
    fail "a restart took a writer the killed process still held (exit ${held_exit})"
fi

# Clearing the lock is an operator's act: the platform will not decide a writer is gone.
find "${WORK}/store" -name '.writer.lock' -delete

CREDENTIALS_DIRECTORY="${WORK}/creds" "$CONTROL_ENGINE" \
    --mode live \
    --operators "${WORK}/operators.json" \
    --config "${WORK}/config.json" --keystore "${WORK}/keystore.bin" --credential drill-cred \
    --store "${WORK}/store" --readiness "${WORK}/run/control-engine.json" \
    --drill-seconds 0 >"${WORK}/ce3.log" 2>&1
restart_exit=$?

if [ "$restart_exit" = 0 ] && grep -qi 'trading halted' "${WORK}/run/control-engine.json"; then
    say "PASS  a restart replays, verifies, and comes back halted rather than resumed"
else
    fail "the restart did not come back halted (exit ${restart_exit})"
fi


kill -9 "$data_pid" 2>/dev/null
wait "$data_pid" 2>/dev/null

if [ "$status" = 0 ]; then
    say "PASS  an interrupted start leaves the platform halted, with no step responsible"
else
    say
    say "The failure value is supposed to be the resting value. If a step has to run for the"
    say "platform to be safe, that step can be skipped - which is the failure this drills for."
    sed 's/^/        control-engine: /' "${WORK}/ce.log" | head -5
fi

exit "$status"
