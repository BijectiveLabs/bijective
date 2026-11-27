# Overview

Bijective is a self-hosted platform for running trading strategies across asset classes and
venues: crypto exchanges and protocols, prediction markets and US equity brokers.
The operator brings the strategy and sets the limits; the platform carries the market data, the
risk gate, the order path and the record of every decision.

## What the platform guarantees by construction

| Guarantee | How it is held |
|---|---|
| No order reaches a venue without passing risk evaluation | `submit` requires a `Clearance` that only a passing evaluation mints |
| A node never trades because it started | The control engine starts `Halted`; resuming is an authorised, recorded command |
| Every decision can be reconstructed | Intents are written before the venue call; every event is in an append-only, hash-chained store |
| Replays reproduce exactly | One clock interface, single-writer streams, no floating point in the record |
| A strategy cannot reach a credential, a venue or the network | Strategies run as separate sandboxed processes and only emit proposals |
| An unknown order outcome is not treated as a failure | Unknown outcomes keep their reservation and are reconciled against the venue |
| Missing data stops trading | Missing or stale inputs and unevaluable limits refuse orders; undeclared integrations report `UNMONITORED` |

## How a decision travels

```
venue -> adapter -> data engine -> market stream
                                       |
                    strategy run (sandboxed) reads the stream, emits proposals
                                       |
            control engine: admission -> risk evaluation -> clearance -> intent recorded
                                       |
                              adapter submit -> venue answer -> settlement recorded
                                       |
                    reconciliation, attribution and the console read the record
```

## Where to go next

- [Architecture](architecture.md): the programs and the dependency rules between them.
- [Order path](order_path.md) and [Risk and limits](risk_and_limits.md): how a proposal becomes an
  order, or does not.
- [Strategies](strategies.md): the protocol, the sandbox, backtest and live sessions.
- [Getting started](../getting_started/index.md): run a first strategy.
