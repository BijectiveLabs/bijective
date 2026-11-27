#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# A node installed per runbook 02 from the shipped, unedited units. Refuses to run if anything it installs already exists.
# A program under a shipped unit starts and writes, and the same unit without its grant refuses, so the first is evidence and not coincidence.
# Needs root without a prompt. Exit codes: 0 a node comes up from the shipped units, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"

OPT=/opt/bijective
STATE=/var/lib/bijective
RUNTIME=/run/bijective
UNITS=/etc/systemd/system
DRILL_UNIT=bijective-node-drill.service
UNGRANTED_UNIT=bijective-node-drill-ungranted.service
CONTROL_UNIT=bijective-control-drill.service

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

installed=0
teardown() {
    if [ "$installed" = 1 ]; then
        sudo -n systemctl stop "$DRILL_UNIT" "$UNGRANTED_UNIT" "$CONTROL_UNIT" >/dev/null 2>&1
                # Disabled before the files go: disable reads [Install] from the unit, and removing it first leaves symlinks behind.
        sudo -n systemctl disable bijective-data-engine.service bijective-control-engine.service \
            "$DRILL_UNIT" "$UNGRANTED_UNIT" "$CONTROL_UNIT" >/dev/null 2>&1
        sudo -n rm -f "${UNITS}/bijective-"*.service "${UNITS}/bijective-"*.timer
        sudo -n find "${UNITS}" -name 'bijective-*' -delete >/dev/null 2>&1
        sudo -n rm -rf "$OPT" "$STATE" "$RUNTIME" /etc/bijective
        sudo -n systemctl daemon-reload >/dev/null 2>&1
        sudo -n systemctl reset-failed >/dev/null 2>&1

                # Printed so an operator can see the host is as it was.
        local left
        left="$(sudo -n find "${UNITS}" -name 'bijective-*' 2>/dev/null | head -3)"
        if [ -n "$left" ] || [ -e "$OPT" ] || [ -e "$STATE" ] || [ -e "$RUNTIME" ] \
            || [ -e /etc/bijective ]; then
            say "FAIL  something was left behind: ${left:-a directory under /opt, /var/lib or /run}"
            status=1
        else
            say "CLEAN the host is as it was: no units, no files, nothing loaded"
        fi
    fi
    rm -rf "$WORK"
}
trap teardown EXIT

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH"; exit 1; }
if ! sudo -n true 2>/dev/null; then
    say "NOT VERIFIED  this needs root without a prompt, and does not have it. Nothing was touched."
    exit 1
fi

# Refuses rather than adopts: anything already at these paths belongs to another installation, and cleanup would remove it.
for path in "$OPT" "$STATE" "$RUNTIME" /etc/bijective; do
    if [ -e "$path" ]; then
        say "FAIL  ${path} already exists; this will not install over a node it did not create"
        exit 1
    fi
done
if compgen -G "${UNITS}/bijective-*.service" >/dev/null; then
    say "FAIL  ${UNITS} already holds bijective units; this will not install over them"
    exit 1
fi

for target in crates/deployment crates/deployment crates/control_engine crates/adapters/registry; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build"; printf '%s\n' "$out" | tail -3; exit 1
    fi
done
# Fixtures the Control Engine needs, built from the current types rather than kept as files.
for target in crates/control_engine crates/secrets_broker crates/adapters/registry; do
    (cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>/dev/null)
done

DEPLOY="${CORE}/target/debug/bijective-deploy"
MANIFEST="${CORE}/crates/deployment/deployment.json"
VERSION="$(python3 -c "import json;print(json.load(open('${MANIFEST}'))['platform_version'])")"

ART="${WORK}/artefact"
mkdir -p "${ART}/bin" "${ART}/units" "${ART}/lib/python"
cp "${CORE}/target/debug/bijective-operational-host" "${ART}/bin/"
cp "$MANIFEST" "${ART}/"
"$DEPLOY" generate "$MANIFEST" "${ART}/units" >/dev/null 2>&1
cp -r "${CORE}/python/bijective-strategy/src/bijective_strategy" "${ART}/lib/python/"
"$DEPLOY" record "$ART" "$VERSION" >/dev/null 2>&1

if out="$("$DEPLOY" verify-artefact "$ART" 2>&1)" && printf '%s' "$out" | grep -q '^PASS'; then
    say "PASS  ${out#PASS  }"
else
    fail "the artefact did not verify, so nothing was installed: ${out}"
    exit 1
fi

installed=1
sudo -n mkdir -p "${OPT}/bin" "${OPT}/lib" "${STATE}/store" "$RUNTIME"
sudo -n cp "${ART}/bin/"* "${OPT}/bin/"
sudo -n cp -r "${ART}/lib/python" "${OPT}/lib/"
sudo -n cp "${ART}/units/"*.service "${ART}/units/"*.timer "$UNITS" 2>/dev/null
sudo -n systemctl daemon-reload

requires="$(sudo -n systemctl show -p Requires --value bijective-control-engine.service 2>/dev/null)"
delegate="$(sudo -n systemctl show -p Delegate --value bijective-strategy-engine.service 2>/dev/null)"
if printf '%s' "$requires" | grep -q 'bijective-data-engine.service' && [ "$delegate" = "yes" ]; then
    say "PASS  the init system registered the ordering and the supervisor's delegated subtree"
else
    fail "the unit set did not land as written (Requires=${requires}, Delegate=${delegate})"
fi

sudo -n systemctl enable bijective-data-engine.service bijective-control-engine.service >/dev/null 2>&1
state="$(sudo -n systemctl is-enabled bijective-strategy-engine.service 2>&1)"
if [ "$state" = "disabled" ] || [ "$state" = "static" ]; then
    say "PASS  the supervisor is installed and not enabled (${state}), so no session begins on boot"
else
    fail "the supervisor is ${state}; a session would begin because a node booted"
fi

# 4: the operational host, because it needs only a store and a readiness file. The engines need a venue and a credential.
sudo -n tee "${UNITS}/${DRILL_UNIT}" >/dev/null <<UNIT
$(sed -e "s|^ExecStart=.*|ExecStart=${OPT}/bin/bijective-operational-host --store ${STATE}/store --readiness ${RUNTIME}/node-drill.json --drill-seconds 6|" \
      -e "s|^Description=.*|Description=Bijective node drill (${VERSION})|" \
      "${ART}/units/bijective-operational-host.service")
UNIT
sudo -n systemctl daemon-reload
sudo -n systemctl start "$DRILL_UNIT" >/dev/null 2>&1

ready=""
for _ in $(seq 1 100); do
    if sudo -n test -s "${RUNTIME}/node-drill.json"; then ready="yes"; break; fi
    sleep 0.1
done
if [ -n "$ready" ]; then
    say "PASS  a program under a shipped unit started and wrote its readiness at ${RUNTIME}"
else
    fail "nothing was written to ${RUNTIME} under the shipped unit"
    sudo -n systemctl status "$DRILL_UNIT" --no-pager 2>&1 | tail -6 | sed 's/^/      /'
fi

sudo -n tee "${UNITS}/${UNGRANTED_UNIT}" >/dev/null <<UNIT
$(grep -v '^ReadWritePaths=' "${UNITS}/${DRILL_UNIT}" \
    | sed -e "s|node-drill.json|node-drill-ungranted.json|" \
          -e "s|^Description=.*|Description=Bijective node drill, ungranted (${VERSION})|")
UNIT
sudo -n systemctl daemon-reload
sudo -n systemctl start "$UNGRANTED_UNIT" >/dev/null 2>&1
sleep 1

if sudo -n test -s "${RUNTIME}/node-drill-ungranted.json"; then
    fail "a unit with no granted path wrote anyway, so property 4 proves nothing here"
else
    say "PASS  and the same unit without its grant wrote nothing - the grant is what makes it work"
fi

# 6: Control Engine under its shipped unit with a credential valid nowhere. A live key has no place in a start-up rehearsal.
# Proves the whole path (init passes the credential, broker opens the keystore, engine replays limit state) with no route to capital.
CONTROL="${CORE}/target/debug/bijective-control-engine"
if [ ! -x "$CONTROL" ]; then
    fail "the Control Engine was not built, so its unit was not exercised"
else
    # shellcheck source=scripts/verify/drill_fixtures.sh
    . "${CORE}/scripts/verify/drill_fixtures.sh"
    VENUE="$("${CORE}/target/debug/examples/list_venues" 2>/dev/null | head -1)"
    if [ -z "$VENUE" ]; then
        fail "the build registers no venue, so no configuration can name one"
    else
        drill_credentials_dir "${WORK}/creds" bijective-init
        drill_profile "$CORE" "$VENUE" "BTCUSDT" "${WORK}/profile.json"
        drill_keystore "$CORE" "${WORK}/creds" bijective-init "${WORK}/keystore.bjks"
        drill_config "$VENUE" "/etc/bijective/profile.json" "${VENUE}.BTCUSDT" "${WORK}/config.json"
        drill_operators "${WORK}/operators.json"

        sudo -n mkdir -p /etc/bijective/credentials
        sudo -n cp "${WORK}/config.json" "${WORK}/profile.json" "${WORK}/operators.json" /etc/bijective/
        sudo -n cp "${WORK}/creds/bijective-init" /etc/bijective/credentials/
        sudo -n cp "${WORK}/keystore.bjks" "${STATE}/keystore.bjks"
        sudo -n cp "$CONTROL" "${OPT}/bin/"

        sudo -n tee "${UNITS}/${CONTROL_UNIT}" >/dev/null <<UNIT
$(sed -e "s|^ExecStart=\(.*\)|ExecStart=\1 --drill-seconds 8|" \
      -e "s|^Description=.*|Description=Bijective control drill (${VERSION})|" \
      -e "/^Requires=/d" -e "/^After=/d" \
      "${ART}/units/bijective-control-engine.service")
UNIT
        sudo -n systemctl daemon-reload
        sudo -n systemctl start "$CONTROL_UNIT" >/dev/null 2>&1

        halted=""
        for _ in $(seq 1 150); do
            if sudo -n grep -q 'halted' "${RUNTIME}/control-engine.json" 2>/dev/null; then
                halted="yes"; break
            fi
            sleep 0.1
        done
        if [ -n "$halted" ]; then
            say "PASS  the Control Engine came up under its shipped unit, and came up halted"
        else
            fail "the Control Engine did not come up halted under its unit"
            sudo -n systemctl status "$CONTROL_UNIT" --no-pager 2>&1 | tail -6 | sed 's/^/      /'
            sudo -n cat "${RUNTIME}/control-engine.json" 2>/dev/null | head -2 | sed 's/^/      /'
        fi
    fi
fi

if [ "$status" = 0 ]; then
    say "PASS  a node comes up from the shipped units, and is taken away again"
fi
exit "$status"
