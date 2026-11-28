#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Assembles a release and verifies it with the code that will verify it on an operator's host (runbook 01).
# A changed byte, a file added after the cut and a listed file gone are each refused. The artefact carries the Python protocol library.
# Exit codes: 0 the release path holds, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH, so the release path is UNVERIFIED"; exit 1; }

# Every crate whose program the artefact carries (scripts/release/assemble.sh).
for target in crates/deployment crates/strategy_engine crates/evaluation crates/control_engine \
    crates/data_engine crates/venue_assurance crates/console crates/cli crates/secrets_broker crates/mcp; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

DEPLOY="${CORE}/target/debug/bijective-deploy"
[ -x "$DEPLOY" ] || { say "FAIL  ${DEPLOY} is missing after a successful build"; exit 1; }

VERSION="$(python3 -c "
import json
print(json.load(open('${CORE}/crates/deployment/deployment.json'))['platform_version'])
")"

# Assembled by the script the release workflow runs, so this drill verifies the tree that ships.
ART="${WORK}/artefact"
recorded="$(bash "${CORE}/scripts/release/assemble.sh" debug "$ART" 2>&1)"
if printf '%s' "$recorded" | grep -qE '^recorded [0-9]+ file\(s\)'; then
    say "PASS  ${recorded}"
else
    fail "the artefact could not be recorded: ${recorded}"
    exit 1
fi

if [ -f "${ART}/lib/python/bijective_strategy/__init__.py" ] \
    && grep -q 'lib/python/bijective_strategy/__init__.py' "${ART}/CHECKSUMS.json"; then
    say "PASS  the release carries the Python library, and the manifest names it"
else
    fail "a Python strategy could not import anything from this release"
fi

if out="$("$DEPLOY" verify-artefact "$ART" 2>&1)" && printf '%s' "$out" | grep -q '^PASS'; then
    say "PASS  ${out#PASS  }"
else
    fail "the recorder and the verifier disagree: ${out}"
fi

refused_for() { # <what> <expected fragment>
    local what="$1" fragment="$2" out
    out="$("$DEPLOY" verify-artefact "$ART" 2>&1)"
    if printf '%s' "$out" | grep -q "$fragment"; then
        say "PASS  ${what}"
    else
        fail "${what} - it was not: ${out}"
    fi
}

printf 'x' >> "${ART}/bin/bijective-operational-host"
refused_for "a changed byte is refused before anything is unpacked" "hashes to"
"$DEPLOY" record "$ART" "$VERSION" >/dev/null 2>&1

printf 'anything\n' > "${ART}/lib/python/added-later.py"
refused_for "a file added after the release was cut is refused" "is in the artefact and not in the manifest"
rm -f "${ART}/lib/python/added-later.py"

rm -f "${ART}/bin/bijective-operational-host"
refused_for "a file listed and missing is refused, not passed over" "is listed and absent"

if [ "$status" = 0 ]; then
    say "PASS  a release can be assembled, recorded, and refused when it differs"
fi
exit "$status"
