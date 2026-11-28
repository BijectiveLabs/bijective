# <img src="assets/bijective_banner.svg" alt="Bijective" width="100%">

[![CI](https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml)
[![Licence: Apache-2.0](https://img.shields.io/badge/licence-Apache--2.0-blue)](LICENSE)
![Rust 1.97.1](https://img.shields.io/badge/rust-1.97.1-orange?logo=rust)
![Python 3.12 to 3.14](https://img.shields.io/badge/python-3.12%E2%80%933.14-3776ab?logo=python&logoColor=white)
[![Release](https://img.shields.io/github/v/release/BijectiveLabs/bijective?include_prereleases)](https://github.com/BijectiveLabs/bijective/releases)

[![Website](https://img.shields.io/badge/WEBSITE-1f6feb?style=for-the-badge)](https://www.bijectivelabs.dev/bijective)
[![Docs](https://img.shields.io/badge/DOCS-0e8a5f?style=for-the-badge)](https://docs.bijectivelabs.dev/bijective/)
[![Insight](https://img.shields.io/badge/INSIGHT-7c3aed?style=for-the-badge)](https://www.bijectivelabs.dev/insights)

| Branch   | Version | Build |
| :------- | :------ | :---- |
| `master` | 0.1.0, the latest release | [![build](https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml) |
| `dev`    | development, the default branch | [![build](https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/BijectiveLabs/bijective/actions/workflows/ci.yml) |

| Platform                      | Rust   | Python      | Notes |
| :---------------------------- | :----- | :---------- | :---- |
| Linux (x86_64)                | 1.97.1 | 3.12 to 3.14 | Built and tested in CI on every push |
| Windows 11 through WSL 2      | 1.97.1 | 3.12 to 3.14 | A Linux host with systemd ([run a node on a Windows workstation](docs/how_to/run_a_node_on_a_windows_workstation.md)) |
| Container (Linux)             | 1.97.1 | not included | Data and control engines, no strategy sandbox ([containers](docs/concepts/deployment.md#containers)) |

macOS and native Windows are not supported: the strategy sandbox is built on Linux namespaces and
cgroup v2.

- **Documentation**: [docs.bijectivelabs.dev/bijective](https://docs.bijectivelabs.dev/bijective/), built from [docs/](docs/index.md)
- **Roadmap**: [ROADMAP.md](ROADMAP.md)
- **Security**: [SECURITY.md](SECURITY.md)

## Introduction

Bijective is a self-hosted, multi-asset trading platform: crypto spot and perpetuals on centralised
and decentralised venues, prediction-market outcomes and US equities. It is built so that no
decision reaches a venue without passing risk evaluation, and every decision can be reconstructed
afterwards.

The core is written in Rust. Strategies run as separate, sandboxed processes and may be written in
any language that reads and writes lines of JSON. A Python library is provided. Backtest, paper and
live run the same strategy program through the same order path and write the same record.

The operator owns the node, the keys, the data and the record. Nothing calls home, and every format
the node writes is documented, so everything it holds can be read without anything else from us.

## Status

Early development: version 0.1.0, and interfaces will change before 1.0. Status as of
2026-11-27, with the evidence for each venue on its [integration page](docs/integrations/index.md).

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
| MCP server for research agents | Built: `bijective-mcp` runs the research programs for an agent, read-only by default, with no route to an order (decision 0020) |
| Market history from a client's CSV or JSON Lines file | Built for top-of-book rows (decision 0016, stage 1) |
| Reconciliation, execution cost, failure detection | Reconciliation on a cadence and on venue reports, and an execution cost table against the book at the decision, built. Missing-event evaluation planned |
| Operator console | Built: one console for every run mode (decision 0017), TLS with client certificates, signed commands, tables and an event stream for programs, OpenMetrics |
| Deployment kit | Built: release artefact and systemd units verified, container composition from the same manifest. The release workflow publishes the artefact and the node image from a version tag |

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

## Versioning and releases

> **Bijective is in early development.** Until 1.0, a minor version may change an interface, a
> format or a flag. Every such change is stated in the release
> notes with the action an operator must take.

- **Version numbers** follow [Semantic Versioning](https://semver.org). The platform version is the
  one the deployment manifest declares, and every runbook is bound to it (`Runbooks` gate).
- **Release notes** are in [RELEASES.md](RELEASES.md): what changed, and any action an operator must
  take before or after upgrading.
- **Release artefacts** are published from a version tag by `.github/workflows/release.yml`: the
  binaries, the generated units, the strategy library and a recorded digest of each file, checked
  before anything is unpacked ([verify a release](docs/how_to/verify_a_release.md)), and the node
  image at `ghcr.io/bijectivelabs/bijective`.

### Branches

- `master`: the latest released version, and the most stable branch. Nothing is written to it
  directly: it moves only by a pull request from `dev` that passes CI, at a release.
- `dev`: the default branch, where development happens and pull requests land. Every change
  passes the same gates as a release.

## Engineering practices

What an operator relies on is checked by a machine on every push, not by a reviewer's memory. The
gate runner (`scripts/ci/run_gates.sh`) runs 41 gates, and a gate that cannot run is a failure. The
full list is in [Quality gates](docs/developer_guide/ci_gates.md). Among them:

- **The order path is closed.** An order cannot be submitted without a clearance that only the risk
  evaluation mints, and only a live session writes proposals.
- **Replay is exact.** The same record replays to byte-identical outputs, and the market archive
  rebuilds byte for byte from the journal and survives a killed process.
- **Recovery is drilled.** An interrupted start leaves the platform halted, a backup restores and
  verifies, and the keystore seal holds.
- **No floating point for money**, no `unsafe` code in any crate, and no outbound network connection
  above the adapter contract.
- **Dependencies are accounted for.** Every package in `Cargo.lock` is attested with its licence,
  only permissive licences are accepted, and the third-party notices match the attestation.
- **Documentation is checked against the tree.** Runbooks name drills that exist, the gate table
  lists exactly what the runner executes, and every venue adapter cites its sources with a date.

Technology choices are made by measurement, and the ones examined and not adopted are recorded
with their evidence in [Technology](docs/concepts/technology.md#measured-and-not-adopted).

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

Report a vulnerability privately through GitHub Security Advisories, never in a public issue. Scope,
what to include and the security model are in [SECURITY.md](SECURITY.md).

## Contributing

Contributions follow [CONTRIBUTING.md](CONTRIBUTING.md) and the
[code of conduct](CODE_OF_CONDUCT.md). Pull requests target `dev`. Run the gates before opening one:
`bash scripts/ci/run_gates.sh`.

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
