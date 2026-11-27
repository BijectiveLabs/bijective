#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Installs the repository's git hooks into this working copy.
# .git/hooks is untracked, so a committed hook does nothing until linked. This is an explicit step
# because a tool that writes into .git unasked is a tool people distrust.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
src="${repo_root}/scripts/ci/hooks"
dst="$(git rev-parse --git-path hooks)"

installed=0
for hook in pre-push; do
    [ -f "${src}/${hook}" ] || continue

    if [ -e "${dst}/${hook}" ] && [ ! -L "${dst}/${hook}" ]; then
        echo "install: ${dst}/${hook} exists and is not a link - leaving it alone." >&2
        echo "install: move it aside first if you want the repository's version." >&2
        continue
    fi

    ln -sf "${src}/${hook}" "${dst}/${hook}"
    chmod +x "${src}/${hook}"
    echo "installed: ${hook}"
    installed=$((installed + 1))
done

if [ "$installed" -eq 0 ]; then
    echo "install: nothing installed." >&2
    exit 1
fi

echo
echo "The hook is a convenience. The authority is the workflow, which runs on a clean"
echo "checkout where local state cannot make a failing gate pass."
