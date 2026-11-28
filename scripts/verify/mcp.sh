#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# A research agent's session against the real programs, through the MCP server: initialize, write a
# strategy, run it, read its report and a console table, and be refused a path outside the workspace.
# Exit codes: 0 every step answered as an agent needs, 1 one did not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! out="$(cd "$CORE" && cargo build --quiet --offline -p bijective-mcp -p bijective-console \
        -p bijective-strategy-engine -p bijective-evaluation 2>&1)"; then
    say "FAIL  the programs did not build, so the MCP server is UNVERIFIED"
    printf '%s\n' "$out" | head -5
    exit 1
fi

{
    echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"drill","version":"1"}}}'
    echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    echo '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
    echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"new_strategy","arguments":{"path":"alpha.py"}}}'
    echo '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"backtest","arguments":{"strategy":"alpha.py"}}}'
    echo '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"report","arguments":{"store":"store"}}}'
    echo '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"panel","arguments":{"store":"store","panel":"strip"}}}'
    echo '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"report","arguments":{"store":"/etc"}}}'
} | "${CORE}/target/debug/bijective-mcp" --workspace "$WORK" --allow-backtest --bound-seconds 300 \
    >"${WORK}/replies.jsonl" 2>"${WORK}/server.log"

if verdict="$(python3 - "${WORK}/replies.jsonl" <<'PY'
import json, sys
replies = {m["id"]: m for m in map(json.loads, open(sys.argv[1])) if "id" in m}
def text(i):
    return replies[i]["result"]["content"][0]["text"]
def ok(i):
    return replies[i]["result"]["isError"] is False
checks = [
    ("the server answered initialize with its tools capability", "tools" in replies[1]["result"]["capabilities"]),
    ("tools/list offered the backtest the operator allowed", "backtest" in [t["name"] for t in replies[2]["result"]["tools"]]),
    ("a strategy was written in the workspace", ok(3) and "alpha.py" in text(3)),
    ("the strategy ran in the sandbox", ok(4) and "bounded" in text(4)),
    ("the report lists the run", ok(5) and "alpha" in text(5)),
    ("a console table came back in the open table format", ok(6) and json.loads(text(6))["format"] == 1),
    ("a path outside the workspace was refused", replies[7]["result"]["isError"] is True and "outside the workspace" in text(7)),
]
for name, passed in checks:
    print(("PASS  " if passed else "FAIL  ") + name)
sys.exit(0 if all(p for _, p in checks) else 1)
PY
)"; then
    printf '%s\n' "$verdict"
else
    printf '%s\n' "$verdict"
    status=1
fi

[ "$status" -eq 0 ] || { fail "the agent session did not hold"; head -5 "${WORK}/server.log"; }
exit "$status"
