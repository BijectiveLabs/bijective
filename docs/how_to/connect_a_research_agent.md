# Connect a research agent

Platform-Version: 0.1.0
Drill: scripts/verify/mcp.sh
Applies-To: a researcher who wants an AI agent to write, run and judge strategies on a node

**Why:** an agent that researches through the platform runs its strategies the way production
does: in the sandbox, over a recorded market, through the same order path and the same evaluation.
`bijective-mcp` is a Model Context Protocol server, so any agent that speaks MCP can use it
([decision 0020](../decisions/0020-mcp-server-for-research-agents.md)).

## What the agent can do

| Tool | What it does | Offered |
|---|---|---|
| `platform` | Says what the server offers, the workspace and the programs it found | Always |
| `panels`, `panel` | Lists and reads the console tables of a record: orders, fills, positions, risk, market data, strategy performance | Always |
| `report` | The evaluation report of the strategies that ran against a record, and one strategy's verdict | Always |
| `promotion` | How far a strategy has come towards capital, and what stops it | Always |
| `new_strategy` | Writes a strategy that already runs, and seeds a sample market | With `--allow-backtest` |
| `backtest` | Runs a strategy in the sandbox over a recorded market, through the order path with `config` | With `--allow-backtest` |

What it cannot do: start a paper or live session, sign an operator command, read a credential or
place an order. The server links no engine and holds no key, and the `Agent environment` gate
refuses any dependency that would change that. Promotion to capital stays an operator's signed act.

## Procedure

1. Choose the workspace, the one directory the agent may read and write. Every path a tool
   receives is resolved there, and a path outside it is refused.
2. Register the server with the agent. For a client configured by JSON:

   ```json
   {
     "mcpServers": {
       "bijective": {
         "command": "bijective-mcp",
         "args": ["--workspace", "/home/you/research", "--allow-backtest"]
       }
     }
   }
   ```

   For Claude Code: `claude mcp add bijective -- bijective-mcp --workspace /home/you/research --allow-backtest`.

3. Ask the agent to call `platform` first. It reports what it found and what it may do.

`bijective-mcp` runs the programs that sit beside it, then those on `PATH`, so a release runs the
programs it was shipped with. A program runs at most `--bound-seconds` (600 by default) and is
then ended with everything it started.

## Reading the result

| Drill line | Meaning |
|---|---|
| `PASS  the strategy ran in the sandbox` | The agent's strategy ran bounded, with no network and no credential |
| `PASS  a console table came back in the open table format` | The agent reads what a person reads in the console |
| `PASS  a path outside the workspace was refused` | The workspace is the boundary of what the agent touches |
