<p align="center"><img src="assets/bijective_banner.svg" alt="Bijective" width="720"></p>

<p align="center">
  <a href="https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml"><img src="https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/licence-Apache--2.0-blue" alt="Licence: Apache-2.0"></a>
  <img src="https://img.shields.io/badge/rust-1.97.1-orange" alt="Rust 1.97.1">
  <img src="https://img.shields.io/badge/status-pre--release-lightgrey" alt="Status: pre-release">
</p>

# Bijective

A self-hosted, multi-asset trading platform: crypto spot and perpetuals on centralised and
decentralised venues, prediction-market outcomes and US equities. It is built so that no decision
reaches a venue without passing risk evaluation, and every decision can be reconstructed
afterwards.

The core is written in Rust. Strategies run as separate, sandboxed processes and may be written in
any language that reads and writes lines of JSON. A Python library is provided.

## Status

Early development. No version has been released and interfaces will change. Status as of
2026-11-27.

| Area | State |
|---|---|
| Event store, clock, bus, deterministic replay | Built |
| Risk limits at six scope levels, safe states, order path with mandatory clearance | Built |
| Order terms: time in force, post-only, stop orders, linked orders, expiry at a time, price collar, flatten | Built (decisions 0009 to 0014) |
| Bybit adapter | Market data, account read, private streams, submit, cancel and stop orders exercised end to end through a node |
| Alpaca adapter (US equities) | Market data, session gate, submit, cancel, evidence and stop orders exercised end to end through a node |
| Hyperliquid adapter | Market data verified against the live venue. Signing, account read, submit, cancel and evidence built, not yet exercised against the venue |
| Polymarket adapter | Market data verified against the live venue. Signing, submit, cancel and evidence built, not yet exercised against the venue |
| Simulated venue, paper runs | Built: orders, fills, positions and reconciliation run the live code with no order leaving the node. A paper venue is rebuilt from its record at start (decision 0018) and reconciled once more when the run stops |
| Backtest | Built: proposals measured against the recorded market and, given a node configuration, placed on simulated venues through the order path (decision 0015, stage 1). The strategy is not yet told of its fills in a backtest (stage 2) |
| Strategy sandbox, live sessions, promotion stages | Built (Linux, cgroup v2) |
| Market history from a client's CSV or JSON Lines file | Built for top-of-book rows (decision 0016, stage 1) |
| Reconciliation, execution cost, failure detection | Reconciliation on a cadence and on venue reports, and an execution cost table against the book at the decision, built. Missing-event evaluation planned |
| Operator console | Built: one console for every run mode (decision 0017), TLS with client certificates, signed commands, tables and an event stream for programs, OpenMetrics |
| Deployment kit | Built: release artefact and systemd units verified, container composition from the same manifest. The release workflow builds the artefact and the node image on demand. Nothing is published yet |

## Design commitments

- **Risk is a gate, not a library.** Submitting an order requires a clearance that only a passing
  risk evaluation can mint. There is no code path from a strategy to a venue that skips it.
- **Fail closed.** A node starts halted. Missing inputs, inputs older than the declared staleness
  bound, and limits that cannot be evaluated stop orders.
- **Events are the source of truth.** State is rebuilt by replaying an append-only, hash-chained
  event store. The same inputs replay to byte-identical outputs.
- **No floating point for money.** Prices, quantities and amounts are scaled integers end to end.
- **Venues are adapters.** Nothing above the adapter contract names a venue. Adding a venue means
  adding one crate.
- **A timeout is unknown, not failed.** An order whose outcome is unknown is reconciled against the
  venue, never assumed.
- **One strategy code path.** Backtest, paper and live run the same strategy program through the
  same supervisor and write the same record.

## Repository layout

```
assets/           Images used by the documentation
crates/           Rust crates: core, adapters, engines and operational tooling
python/           Python packages: the strategy protocol library, attribution, detection
docs/             Documentation
scripts/ci/       Change gates run on every push and pull request
scripts/release/  Release artefact assembly
scripts/verify/   End-to-end drills
scripts/measure/  Measurement scripts
```

## Quick start

The platform targets Linux. Strategy runs are bounded with cgroup v2 and Linux namespaces.

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"

bijective-backtest --new alpha.py   # write a strategy that runs, and seed a sample market
bijective-backtest alpha.py         # run it, bounded by the sandbox
bijective-promote store alpha.py 19600
```

See [Getting started](docs/getting_started/index.md) for requirements and what each step does,
and [Run a first strategy on a live market](docs/how_to/run_a_first_strategy_on_a_live_market.md)
for a paper run on a live public market with no venue key.

## Console

<p><img src="assets/console.svg" alt="Console" width="240"></p>

`bijective-console` serves the operator's view of one run: live, paper or backtest (`--mode`). It
binds loopback unless it is given HTTPS with client certificates, signs operator commands for the
node whose record it shows, and answers every panel as a table at `/api/panels`, as an event
stream at `/api/events` and as OpenMetrics at `/metrics`. See
[Observability](docs/concepts/observability.md) and
[Reach the console from another machine](docs/how_to/reach_the_console.md).

## Documentation

- [Getting started](docs/getting_started/index.md)
- [Concepts](docs/concepts/index.md)
- [How-to guides](docs/how_to/index.md)
- [Integrations](docs/integrations/index.md)
- [Developer guide](docs/developer_guide/index.md)
- [Decision records](docs/decisions/index.md)
- [Roadmap](ROADMAP.md)

## Security

Report vulnerabilities as described in [SECURITY.md](SECURITY.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Third-party notices are in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md). Every source file carries its licence notice,
and [TRADEMARKS.md](TRADEMARKS.md) says how the name may be used by a fork or a redistribution.

What is released here stays open:

- A capability released under this licence is never moved behind a paid product, and a released
  version's licence is never changed.
- Safety is never paid for: risk limits, halt, cancel, reconciliation, signed operator commands
  and TLS are part of this repository.
- Formats are open: the event store, the wire formats and the market archive are documented, so
  everything a node holds can be read without anything else from us.
- Nothing here depends on a Bijective Pro component.
