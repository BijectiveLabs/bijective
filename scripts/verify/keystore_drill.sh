#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Keystore drill: loads a credential as an operator does and proves the seal is real.
# The tool's refusals are demonstrated, not assumed.
# Exit codes: 0 the keystore sealed and every refusal was demonstrated, 1 otherwise.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BROKER="${CORE}/crates/secrets_broker"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH; the drill is UNVERIFIED"; exit 1; }
if ! out="$(cd "$BROKER" && cargo build --quiet --offline 2>&1)"; then
    say "FAIL  the broker did not build, so the drill is UNVERIFIED"
    printf '%s\n' "$out" | sed 's/^/        /' | head -5
    exit 1
fi

TOOL="${CORE}/target/debug/bijective-secrets"
KEYSTORE="${WORK}/keystore.bjks"
PASS="a-drill-passphrase"
SECRET="a-drill-secret-value"

printf '%s\n' "$PASS" | "$TOOL" init "$KEYSTORE" >/dev/null 2>&1 \
    || { fail "the keystore could not be created"; exit 1; }
printf '%s\n%s\n' "$PASS" "$SECRET" | "$TOOL" put "$KEYSTORE" drill-credential >/dev/null 2>&1 \
    || { fail "the credential could not be stored"; exit 1; }

names="$(printf '%s\n' "$PASS" | "$TOOL" list "$KEYSTORE" 2>/dev/null)"
case "$names" in
    *drill-credential*) say "PASS  a credential loaded through the tool is listed back" ;;
    *) fail "the stored credential is not listed" ;;
esac

# Plaintext surviving on disk would defeat the seal.
if grep -q "$SECRET" "$KEYSTORE" 2>/dev/null; then
    fail "the secret is readable in the keystore file"
else
    say "PROVEN the secret is not readable in the sealed file"
fi

perms="$(stat -c '%a' "$KEYSTORE" 2>/dev/null)"
[ "$perms" = "600" ] || fail "the keystore is mode ${perms}, not 600"

if printf '%s\n' "wrong-passphrase" | "$TOOL" list "$KEYSTORE" >/dev/null 2>&1; then
    fail "a wrong passphrase opened the keystore"
else
    say "PROVEN a wrong passphrase is refused"
fi

if printf '%s\n' "$PASS" | "$TOOL" init "$KEYSTORE" >/dev/null 2>&1; then
    fail "init overwrote an existing keystore"
else
    say "PROVEN init refuses to replace an existing keystore"
fi

if printf '\n' | "$TOOL" list "$KEYSTORE" >/dev/null 2>&1; then
    fail "an empty passphrase was accepted"
else
    say "PROVEN an empty passphrase is refused"
fi

# The engines open only a keystore sealed under the init credential their units load. The tool
# must be able to make one, or an operator who follows the steps has a node that cannot start.
CRED_DIR="${WORK}/credstore"
mkdir -p "$CRED_DIR"
head -c 32 /dev/urandom > "${CRED_DIR}/node-keystore"
NODE_KEYSTORE="${WORK}/node.bjks"
INIT=(--init-credential "${CRED_DIR}/node-keystore")
"$TOOL" "${INIT[@]}" init "$NODE_KEYSTORE" </dev/null >/dev/null 2>&1 \
    || fail "a keystore could not be created under the init credential"
printf '%s\n' "$SECRET" | "$TOOL" "${INIT[@]}" put "$NODE_KEYSTORE" venue-api-key >/dev/null 2>&1 \
    || fail "a credential could not be stored under the init credential"
case "$("$TOOL" "${INIT[@]}" list "$NODE_KEYSTORE" </dev/null 2>/dev/null)" in
    *venue-api-key*) say "PASS  a keystore sealed under the init credential opens as the engines open it" ;;
    *) fail "the keystore sealed under the init credential did not open" ;;
esac
if "$TOOL" "${INIT[@]}" list "$KEYSTORE" </dev/null >/dev/null 2>&1; then
    fail "a keystore sealed under a passphrase opened under the init credential"
else
    say "PROVEN a keystore sealed under a passphrase is refused where the engines read"
fi
head -c 32 /dev/urandom > "${CRED_DIR}/other"
if "$TOOL" --init-credential "${CRED_DIR}/other" list "$NODE_KEYSTORE" </dev/null >/dev/null 2>&1; then
    fail "another init credential opened the keystore"
else
    say "PROVEN another init credential is refused"
fi

if [ "$status" = 0 ]; then
    say "PASS  a credential loads through the broker and the seal holds"
else
    say
    say "A credential that can be loaded but not sealed, or sealed but not refused to the wrong"
    say "passphrase, is a file with extra steps."
fi

exit "$status"
