# Roadmap

Priorities change as the platform is built and operated. This page states direction, not dates.
It reflects the tree as of 2026-11-27.

## Built

- The core: clock, bus, event store, codec, projections and the determinism harness.
- Risk: limits at six scope levels (decision 0001), durable limit state, valuation, desks and
  admission, safe states and the pre-trade check.
- The order path with a mandatory clearance, budget reservation and the operator command path.
- The strategy protocol, sandbox, supervisor, bounded runs, live sessions and the run ledger.
- Evaluation: pre-registration, walk-forward, sufficiency, cost measurement and the promotion stages.
- Venue adapters for Bybit, Hyperliquid, Polymarket and Alpaca: market data, signed order calls,
  venue evidence for reconciliation, and private order streams that prompt an evidence read.
  Bybit and Alpaca execution is exercised end to end through a node. Hyperliquid and Polymarket
  execution is built and not yet exercised against the venue
  ([integrations](docs/integrations/index.md)).
- Multi-asset scope (decision 0004): crypto spot and perpetuals, prediction-market outcomes and US
  equities, with a session gate for venues that trade in sessions.
- Reconciliation on a cadence and on venue reports, restart recovery from the record, decision
  capture, attribution and failure detection in part.
- Paper runs on simulated venues with the live order path. A paper venue is rebuilt from its record
  at start, so a restarted paper run continues (decision 0018), and a paper run reconciles once
  more and records every simulated fill when it stops.
- Market data capture beyond top of book: trades, funding, open interest, mark and index prices and
  liquidations, archived as Parquet with zstd compression, compaction and gap marking.
- One risk book in one reporting currency across settlement currencies (decision 0005), with the
  book's equity recorded in each portfolio summary.
- Order terms and protection: time in force, post-only, stop orders, linked orders, expiry at a
  time, a price collar and an operator flatten (decisions 0009 to 0014).
- A backtest through the order path on simulated venues, its passes placed by the recording
  (decision 0015, stage 1).
- Market history from a client's CSV or JSON Lines file, read as a recording (decision 0016,
  stage 1).
- A first run with no key: a venue profile drafted from the venue's published rules and one
  command for a paper run on a live public market.
- One console for every run mode (decision 0017): the same panels over a live, paper or backtest
  record, TLS with client certificates, signed commands, every panel as a table and as an event
  stream, OpenMetrics, an execution cost table against the book at the decision, and strategy
  performance of every run side by side.
- The deployment kit, readiness, the change gates, and a release workflow that builds, verifies
  and, on a version tag, publishes the release artefact, the strategy library and the node image.
- Market capture sized by measurement: the market journal is made durable per batch of what is
  already waiting (decision 0019), and the technologies examined and not adopted are recorded
  with their evidence ([Technology](docs/concepts/technology.md)).
- An MCP server for research agents (decision 0020): an agent writes, runs and judges strategies
  through the research programs, with no route to an order
  ([connect a research agent](docs/how_to/connect_a_research_agent.md)).
- Thirty-four crates along the architectural layers, each with one reason to change.

## Next

1. **Order feedback in a backtest.** The strategy told of its orders and fills at the position it
   would be told in paper and live (decision 0015, stage 2).
2. **Packages on registries.** The strategy library on PyPI, and a `bijective` package that installs
   the programs, so a first run needs neither a build nor a download by hand.
3. **More of your own data.** Trades, funding, mark prices, open interest, Parquet input and
   OHLCV bars from a client's files (decision 0016, stage 2).
4. **Venues.** Exercise Hyperliquid and Polymarket execution end to end against the venue, then
   add OKX, Binance and Interactive Brokers as native adapters.
5. **Market state.** A point-in-time, cross-venue market state, versioned and auditable, that
   strategies, the risk engine and models can consume in the same form in backtest and live.
6. **Exchange calendars** for venues that trade in sessions, so simulated and backtest runs keep
   the same hours as the venue.
7. **Proactive rate limiting** per venue, as configuration rather than reaction.

## Scope

In scope: self-hosted operation by the operator, on infrastructure the operator controls.

Out of scope: hosting or operating a node on an operator's behalf, holding an operator's keys, and
providing trading strategies.
