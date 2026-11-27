# Releases

Each release records its version, what changed, and any action an operator must take before or
after upgrading. A release is published from a version tag by `.github/workflows/release.yml`: a
GitHub release with the artefact, its SHA-256 digest and the strategy library, and the node image
at `ghcr.io/bijectivelabs/bijective`. Verify an artefact before installing it with
[Verify a release before installing it](docs/how_to/verify_a_release.md).

## 0.1.0

The first release. Interfaces, formats and flags may change in a later 0.x release, and each such
change will be stated here with the action it needs.

What it contains is listed in the [README status](README.md#status) and the
[roadmap](ROADMAP.md). In short: the event store and deterministic replay, risk limits at six
scope levels with a mandatory clearance for every order, adapters for Bybit, Alpaca, Hyperliquid and
Polymarket, paper and backtest runs on simulated venues through the live order path, the strategy
sandbox, the operator console and the deployment kit.

Assets:

- `bijective-0.1.0-linux-x86_64.tar.gz` and its `.sha256`: the programs, the generated units and the
  strategy library, with a recorded digest of every file.
- `bijective_strategy-0.1.0` wheel and source archive: the Python strategy library, standard library
  only.
- `ghcr.io/bijectivelabs/bijective:0.1.0`: the node image.

Operator action: none, this is the first release.
