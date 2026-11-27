#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Refuses any crate whose resolved dependency tree carries a copyleft-only package.
#
# The platform admits no copyleft component at runtime, and its licence gives
# the rule a second basis. The failure it guards is one line in a manifest, with nothing at
# build time to mark it, so the check is mechanical. A package that offers a permissive
# alternative (for example MIT OR LGPL) passes. The elected licence is recorded separately in
# the dependency attestation.
#
# Exit codes: 0 clean, 1 a copyleft-only package was found or the tree could not be resolved.
# An unresolvable tree is a failure, never a skip: adding a dependency that cannot be resolved
# is exactly what happens when someone introduces one.

set -euo pipefail

CRATE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

copyleft_only() {
    python3 -c '
import json, re, sys
for pkg in json.load(sys.stdin)["packages"]:
    expr = pkg.get("license") or ""
    alternatives = [a for a in re.split(r"\s+OR\s+|/", expr) if a.strip()]
    if alternatives and all(re.search(r"GPL", a, re.I) for a in alternatives):
        print(pkg["name"], pkg["version"], expr)
'
}

status=0

# Walked, not globbed: the tree is nested, and a one-level glob once matched nothing while
# still exiting clean.
while IFS= read -r manifest; do
    grep -q '^\[package\]' "$manifest" || continue
    crate_dir="$(dirname "$manifest")"
    crate_name="${crate_dir#"${CRATE_ROOT}/"}"

    if ! meta="$(cd "$crate_dir" && cargo metadata --offline --format-version 1 2>&1)"; then
        echo "FAIL  ${crate_name}: dependency tree could not be resolved, so linkage is UNVERIFIED"
        echo "$meta" | sed 's/^/        /' | head -5
        status=1
        continue
    fi

    if ! found="$(echo "$meta" | copyleft_only)"; then
        echo "FAIL  ${crate_name}: licence data could not be read, so linkage is UNVERIFIED"
        status=1
        continue
    fi

    if [ -n "$found" ]; then
        echo "FAIL  ${crate_name}: a copyleft-only package is in the dependency tree"
        echo "$found" | sed 's/^/        /'
        status=1
    else
        echo "PASS  ${crate_name}: no copyleft-only package in the dependency tree"
    fi
done < <(find "$CRATE_ROOT" -name Cargo.toml -not -path '*/target/*' | sort)

if [ "$status" -ne 0 ]; then
    echo
    echo "No copyleft component is admitted at runtime in any layer."
fi

exit "$status"
