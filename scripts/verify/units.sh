#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Runs the generated units (scripts/verify/boundary.sh starts programs directly): granted paths, delegated subtree, enable list, start and readiness write.
# Hardening not in force on a host that only runs user units is reported as unverified, not passed over.
# Exit codes: 0 the units carry a node, 1 they do not or it could not be shown here.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
UNIT_NAME="bijective-units-drill.service"
UNIT_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"

cleanup() {
    if [ -f "${UNIT_DIR}/${UNIT_NAME}" ]; then
        systemctl --user stop "$UNIT_NAME" >/dev/null 2>&1
        rm -f "${UNIT_DIR}/${UNIT_NAME}"
        systemctl --user daemon-reload >/dev/null 2>&1
    fi
    rm -rf "${WORK}"
}
trap cleanup EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

for tool in cargo systemctl systemd-analyze; do
    command -v "$tool" >/dev/null 2>&1 || { say "FAIL  no ${tool} on PATH, so the units are UNVERIFIED"; exit 1; }
done

for target in crates/deployment crates/deployment crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

DEPLOY="${CORE}/target/debug/bijective-deploy"
HOST="${CORE}/target/debug/bijective-operational-host"
[ -x "$DEPLOY" ] && [ -x "$HOST" ] || { say "FAIL  a binary is missing after a successful build"; exit 1; }

shipped="${WORK}/shipped"
listed="$("$DEPLOY" generate "${CORE}/crates/deployment/deployment.json" "$shipped" 2>&1)" || {
    say "FAIL  the shipped manifest did not generate: ${listed}"; exit 1; }

# Names come from the manifest, not a copy here: a copy goes stale in the direction that hides a program granted no permission.
granted=0
missing=""
while IFS= read -r name; do
    unit="${shipped}/bijective-${name}.service"
    if [ -f "$unit" ] && grep -q '^ReadWritePaths=' "$unit"; then
        granted=$(( granted + 1 ))
    else
        missing="${missing} ${name}"
    fi
done < <(python3 -c "
import json
d = json.load(open('${CORE}/crates/deployment/deployment.json'))
for p in d['programs']:
    if p.get('writes'):
        print(p['name'])
")

if [ -z "$missing" ] && [ "$granted" -gt 0 ]; then
    say "PASS  every program that declared a writable path was granted it (${granted} unit(s))"
else
    fail "units granted nothing to:${missing:- none, and none were checked}"
fi

supervisor="${shipped}/bijective-strategy-engine.service"
if [ -f "$supervisor" ] \
    && grep -q '^Delegate=yes' "$supervisor" \
    && grep -q -- '--events-behind' "$supervisor"; then
    say "PASS  the supervisor's unit carries its delegated subtree and a declared tolerance"
else
    fail "the supervisor's unit is missing its subtree or its tolerance"
fi

if printf '%s\n' "$listed" | grep -q '^install-stopped bijective-strategy-engine.service$'; then
    say "PASS  and it is installed stopped, so no session begins because a node booted"
else
    fail "the supervisor is enabled at install: $(printf '%s' "$listed" | grep strategy-engine)"
fi

mkdir -p "${WORK}/store" "${WORK}/run" "$UNIT_DIR"
if [ -e "${UNIT_DIR}/${UNIT_NAME}" ]; then
    say "FAIL  ${UNIT_DIR}/${UNIT_NAME} already exists; this drill will not overwrite it"
    exit 1
fi

cat > "${WORK}/drill.json" <<JSON
{
  "version": 1,
  "platform_version": "0.1.0",
  "programs": [
    {
      "name": "units-drill",
      "binary": "${HOST}",
      "class": "core",
      "arguments": [
        "--store", "${WORK}/store",
        "--readiness", "${WORK}/run/units-drill.json",
        "--drill-seconds", "4"
      ],
      "after": [],
      "memory_max_mb": 256,
      "stop_timeout_seconds": 10,
      "writes": ["${WORK}/store", "${WORK}/run"]
    }
  ],
  "checks": []
}
JSON

drilled="${WORK}/units"
"$DEPLOY" generate "${WORK}/drill.json" "$drilled" >/dev/null 2>&1 || {
    say "FAIL  the drill manifest did not generate"; exit 1; }

# Judged on what it says about our file: verify loads the host's whole unit tree, so an unrelated broken unit must not condemn ours.
out="$(systemd-analyze verify "${drilled}/${UNIT_NAME}" 2>&1)"
analyzed=$?
about_ours="$(printf '%s\n' "$out" | grep -F "$UNIT_NAME" || true)"
if [ "$analyzed" -eq 0 ] && [ -z "$about_ours" ]; then
    say "PASS  the init system accepts the generated unit as written"
else
    fail "the init system rejected the generated unit"
    printf '%s\n' "${about_ours:-$out}" | head -4 | sed 's/^/      /'
fi

cp "${drilled}/${UNIT_NAME}" "${UNIT_DIR}/${UNIT_NAME}"
systemctl --user daemon-reload >/dev/null 2>&1
if ! systemctl --user start "$UNIT_NAME" >/dev/null 2>&1; then
    fail "the generated unit would not start"
    systemctl --user status "$UNIT_NAME" --no-pager 2>&1 | tail -5 | sed 's/^/      /'
else
    ready=""
    for _ in $(seq 1 100); do
        if [ -s "${WORK}/run/units-drill.json" ]; then ready="yes"; break; fi
        sleep 0.1
    done
    exit_status="$(systemctl --user show -p ExecMainStatus --value "$UNIT_NAME" 2>/dev/null)"
    if [ -n "$ready" ]; then
        say "PASS  the unit started its program, and the program wrote what it was granted"
    elif [ "$exit_status" = 226 ]; then
        # 226 is the NAMESPACE exit: this user manager applies PrivateTmp and ProtectHome, so the unit
        # cannot see a workspace under /tmp or a build under /home. The hardening is working.
        say "NOT VERIFIED  this user manager applies the unit's mount namespace, so a workspace under"
        say "              /tmp and a build under /home are hidden from it. scripts/verify/node.sh"
        say "              starts the shipped units as system units at the manifest's paths."
        status=1
    else
        fail "the unit started and nothing was written to ${WORK}/run"
        systemctl --user status "$UNIT_NAME" --no-pager 2>&1 | tail -5 | sed 's/^/      /'
    fi
fi

# 5: a program whose store it cannot write must refuse at start, naming the path, not report ready and fail later.
SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
shut="${WORK}/shut"
mkdir -p "$shut"
chmod 500 "$shut"

if ( printf '%s' > "${shut}/probe" ) 2>/dev/null; then
    say "NOT VERIFIED  this process can write a directory it has no permission on, so a store it"
    say "              cannot write is not a state this host can produce"
    status=1
else
    refused="$("$SUPERVISOR" \
        --readiness "${WORK}/run/refusal.json" \
        --cgroup-root "${WORK}/no-such-subtree" \
        --store "$shut" \
        --strategy /bin/true \
        --market "market-drill" \
        --to-position 0 \
        --exchange whole 2>&1)"
    if printf '%s' "$refused" | grep -q "the store at ${shut} cannot be written"; then
        say "PASS  a program whose store it cannot write refuses at start, naming the path"
    else
        fail "a program with an unwritable store did not say so: ${refused}"
    fi
fi
chmod 700 "$shut" 2>/dev/null || true

# 6: asked by trying: a user manager without mount namespace privilege accepts ProtectSystem=strict and applies nothing.
outside="${WORK}/outside"
mkdir -p "${WORK}/inside" "$outside"
chmod 777 "${WORK}" "${WORK}/inside" "$outside"

probe_under() { # <runner...>
    "$@" systemd-run --wait --pipe --quiet \
        -p ProtectSystem=strict -p ReadWritePaths="${WORK}/inside" \
        -- /bin/sh -c "echo x > ${outside}/f 2>/dev/null; echo rc=\$?" 2>&1 | tail -1
}

# User manager first (needs nothing): it applies nothing, so a writable result is expected and proves nothing alone.
probe="$(probe_under env --user)"

# The system manager settles it, only where privilege is already granted. The unit is transient and leaves nothing behind.
manager="user"
if [ "$probe" != "rc=1" ] && [ "$probe" != "rc=2" ] && sudo -n true 2>/dev/null; then
    probe="$(probe_under sudo -n)"
    manager="system"
fi

case "$probe" in
    rc=1|rc=2)
        say "PASS  the hardening is in force (${manager} manager): a write outside the granted paths was refused"
        ;;
    *)
        say "NOT VERIFIED  no manager here applies ProtectSystem to a unit this drill can run, so the"
        say "              granted paths above are declared and unexercised. Run this on a host where"
        say "              the platform's units are system units, or where privilege is available."
        status=1
        ;;
esac

if [ "$status" = 0 ]; then
    say "PASS  the generated units carry a node"
fi
exit "$status"
