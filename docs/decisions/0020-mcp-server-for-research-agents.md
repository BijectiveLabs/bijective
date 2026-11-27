# 0020: An MCP server for research agents

| Field | Value |
|---|---|
| Status | Accepted and built, 2026-11-28 (`crates/mcp`, [connect a research agent](../how_to/connect_a_research_agent.md)) |
| Extends | The research programs (`bijective-backtest`, `bijective-report`, `bijective-promote`), the market archive, the console's tables, the sandbox (decision 0003) |

## Context

Clients already use AI agents for research. An agent that can read the market archive, run a
backtest and read its verdict can iterate on a strategy against the platform's own order path,
instead of against a notebook that behaves differently in production. The Model Context Protocol
(MCP) is the protocol those agents speak to tools: JSON-RPC 2.0 messages, here over the process's
standard input and output.

The constraint is the one every other caller meets: nothing reaches a venue without passing risk
evaluation, and promotion to live is an operator's signed act. An agent is one more caller, not a
new path.

## Decision

1. **One program, `bijective-mcp`, started by the agent's host on the node.** It speaks MCP over
   stdio: `initialize`, `ping`, `tools/list` and `tools/call`. It opens no network port, so who can
   start it is who can log in to the node. Every path a tool receives is resolved inside one
   workspace directory named at start, and a path outside it is refused.
2. **Tools are the research programs, not new logic.** Each tool runs what a person would run and
   returns its output as structured content:

   | Tool | Runs | Writes |
   |---|---|---|
   | `platform` | nothing: what the server offers and which programs it found | nothing |
   | `panels`, `panel` | `bijective-console --serve-one` on loopback, asked for `/api/panels` | nothing |
   | `report` | `bijective-report <store> [<strategy> <confidence>]` | nothing |
   | `promotion` | `bijective-promote <store> <strategy> <confidence>` | nothing |
   | `new_strategy` | `bijective-backtest --new <path>` | a strategy and a sample market in the workspace |
   | `backtest` | `bijective-backtest <strategy> [--market] [--store] [--config]` | the backtest's record |

   The programs run as children, bounded in time, and are ended with everything they started.
   No engine is linked, so the server's dependency graph holds `serde_json` and nothing else.

3. **Read-only by default.** `new_strategy` and `backtest` are offered only when the operator
   starts the server with `--allow-backtest`. A strategy the agent wrote runs in the sandbox like any other: no network, no
   credentials, bounded memory and time.
4. **No route to an order.** The server holds no credential and no operator key. It cannot issue a
   signed command, start a paper or live session, or write a proposal stream. The crate sits in
   the strategy area (`scripts/ci/areas.txt`), so the `Agent environment` gate refuses any
   dependency from it on the secrets broker, a venue, the limits, the risk code or the control
   engine.
5. **The protocol is written by hand over `serde_json`**, which the tree already carries. The
   surface used is three methods and one transport. An SDK is adopted when a second transport or
   protocol feature is needed.

## Consequences

- An agent researches against the exact paths a production strategy takes, and its results are the
  same records a person reads in the console.
- What an agent did is in the record: every backtest it ran writes a directory and a run ledger
  entry, like one a person ran.
- Promotion stays human: the agent can read that a strategy is ready, and an operator signs the
  promotion.

## Rejected

- **An MCP server inside the console.** The console is the one program that listens on the network,
  and an agent's tool calls would then need its TLS and client certificates for no gain on the
  node itself.
- **Tools that submit orders, even paper ones.** A paper session is the step before live and is
  started by an operator. Letting an agent start one would make the agent the operator.
