#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Assembles a release artefact from built programs and records its manifest, in the layout a node
# expects: bin/, units/, lib/python/ and deployment.json. Builds nothing: the caller builds the
# profile it names, so a drill and the release workflow assemble the same tree.
# Usage: assemble.sh <debug|release> <out-dir>. Exit codes: 0 recorded, 1 not, 2 usage.

set -euo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE="${1:-}"
OUT="${2:-}"
case "$PROFILE" in
    debug | release) ;;
    *) echo "usage: assemble.sh <debug|release> <out-dir>" >&2; exit 2 ;;
esac
[ -n "$OUT" ] || { echo "usage: assemble.sh <debug|release> <out-dir>" >&2; exit 2; }

BUILT="${CORE}/target/${PROFILE}"
DEPLOY="${BUILT}/bijective-deploy"
[ -x "$DEPLOY" ] || { echo "${DEPLOY} is missing; build the workspace first" >&2; exit 1; }

VERSION="$(python3 -c "
import json, sys
print(json.load(open(sys.argv[1]))['platform_version'])
" "${CORE}/crates/deployment/deployment.json")"

mkdir -p "${OUT}/bin" "${OUT}/units" "${OUT}/lib/python"
# Programs a node runs and commands an operator runs, so an operator can try a release without
# building it. The control engine beside bijective-backtest runs a backtest's order path
# (decision 0015). The recorder, the importer, the profile drafter and the paper launcher make a
# first run need no key.
for binary in bijective-operational-host bijective-strategy-engine bijective-backtest \
    bijective-control-engine bijective-data-engine bijective-import bijective-profile \
    bijective-paper bijective-promote bijective-report; do
    cp "${BUILT}/${binary}" "${OUT}/bin/"
done
cp "${CORE}/crates/deployment/deployment.json" "${OUT}/"
"$DEPLOY" generate "${CORE}/crates/deployment/deployment.json" "${OUT}/units" >/dev/null
cp -r "${CORE}/python/bijective-strategy/src/bijective_strategy" "${OUT}/lib/python/"
"$DEPLOY" record "$OUT" "$VERSION"
