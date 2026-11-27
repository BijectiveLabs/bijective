#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# The console over HTTPS, driven as a process: a connection without a client certificate is
# refused in the handshake, one with a certificate the operator's authority issued reads the
# control page, a form carrying the page's token is signed into the command stream, and a form
# without it is refused. Certificates are made here with openssl. The console ships none.

set -u
status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
PID=""
trap '[ -n "$PID" ] && kill "$PID" 2>/dev/null; rm -rf "${WORK}"' EXIT

for tool in cargo openssl curl; do
    command -v "$tool" >/dev/null 2>&1 || { say "FAIL  no ${tool} on PATH, so console TLS is UNVERIFIED"; exit 1; }
done
if ! out="$(cd "$CORE" && cargo build --quiet --offline -p bijective-console -p bijective-cli 2>&1)"; then
    say "FAIL  the programs did not build, so console TLS is UNVERIFIED"
    printf '%s\n' "$out" | sed 's/^/        /' | head -5
    exit 1
fi
CONSOLE="${CORE}/target/debug/bijective-console"
CLI="${CORE}/target/debug/bijective"

# An authority, a server certificate for loopback, and one client certificate it issued.
cd "$WORK" || exit 1
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=drill authority" -keyout ca.key -out ca.pem 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=127.0.0.1" \
    -keyout server.key -out server.csr 2>/dev/null
printf 'subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth\n' > server.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 1 \
    -extfile server.ext -out server.pem 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=ana" \
    -keyout client.key -out client.csr 2>/dev/null
printf 'extendedKeyUsage=clientAuth\n' > client.ext
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 1 \
    -extfile client.ext -out client.pem 2>/dev/null
"$CLI" keygen ana operator "$WORK" >/dev/null || { say "FAIL  no operator key"; exit 1; }

PORT=$(( 20000 + RANDOM % 20000 ))
mkdir -p "${WORK}/store"
"$CONSOLE" --store "${WORK}/store" --port "$PORT" --tls-cert server.pem --tls-key server.key \
    --client-ca ca.pem --operator ana --role operator --key "${WORK}/ana.key" 2>"${WORK}/console.log" &
PID=$!
URL="https://127.0.0.1:${PORT}"
for _ in $(seq 1 50); do curl -s -o /dev/null --cacert ca.pem "$URL/" 2>/dev/null; [ $? -ne 7 ] && break; sleep 0.1; done
with_cert() { curl -s --cacert ca.pem --cert client.pem --key client.key "$@"; }

if curl -s -o /dev/null --cacert ca.pem "$URL/"; then
    fail "a connection without a client certificate was answered"
else
    say "PASS  a connection without a client certificate is refused in the handshake"
fi
if curl -s -o /dev/null "http://127.0.0.1:${PORT}/"; then
    fail "plain http was answered on the HTTPS port"
else
    say "PASS  plain http is not answered"
fi

page="$(with_cert "$URL/control")"
token="$(grep -o 'name="token" value="[0-9a-f]*"' <<<"$page" | head -1 | sed 's/.*value="//; s/"//')"
if grep -q 'Signing as' <<<"$page" && [ -n "$token" ]; then
    say "PASS  a client certificate the authority issued reads the control page"
else
    fail "the control page was not served to a certified client"
fi

post() {
    with_cert -o /dev/null -w '%{redirect_url}' -H "Origin: ${URL}" \
        -H 'Content-Type: application/x-www-form-urlencoded' --data "$1" "$URL/control"
}
issued="$(post "token=${token}&action=halt")"
segment="${WORK}/store/commands/000000000000.seg"
if [[ "$issued" == *"/control?issued=0" ]] && [ -s "$segment" ]; then
    say "PASS  a form with the page's token is signed into the command stream"
else
    fail "the halt form was not issued: ${issued:-no redirect}; $(tail -1 "${WORK}/console.log")"
fi
size="$(stat -c %s "$segment" 2>/dev/null || echo 0)"
refused="$(post "token=$(printf '0%.0s' $(seq 1 64))&action=halt")"
if [[ "$refused" == *"refused=token" ]] && [ "$(stat -c %s "$segment")" = "$size" ]; then
    say "PASS  a form without the page's token is refused and appends nothing"
else
    fail "a form without the token was not refused: ${refused:-no redirect}"
fi

if [ "$status" -eq 0 ]; then
    say "PASS  the console serves HTTPS to certified clients only and signs only its own forms"
fi
exit "$status"
