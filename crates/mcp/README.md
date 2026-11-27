# bijective-mcp

The Model Context Protocol server through which an AI agent researches strategies on a node
(decision 0020). It speaks MCP over standard input and output, and each tool runs one of the
platform's research programs inside a workspace directory. It links no engine and holds no
credential: an agent can read records, write strategies and run backtests, and cannot place an
order.

See [Connect a research agent](../../docs/how_to/connect_a_research_agent.md).
