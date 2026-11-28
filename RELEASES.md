# Releases

Each release records its version, what changed, and any action an operator must take before or
after upgrading. A release is published from a version tag by `.github/workflows/release.yml`: a
GitHub release with the artefact, its SHA-256 digest and the strategy library, and the node image
at `ghcr.io/bijectivelabs/bijective`. Verify an artefact before installing it with
[Verify a release before installing it](docs/how_to/verify_a_release.md).

## Unreleased

- **A stale market refuses market orders too.** A market order on a flat book was placed on a
  mark of any age, because the stale-figures check reads the age of held positions and a flat
  book holds none. Every order now needs a mark within the freshness bound (decision 0010,
  amended). A strategy that sends market orders on an instrument the node has no current market
  for is refused where it was placed before.
- **The event store tells a damaged length from a torn tail.** A length field damaged to run past
  the end of a segment read as a torn tail, so the segment verified clean and the entries after it
  were dropped from reads and from recovery. It is now `Corrupt` (`PayloadLengthDamaged`). A sealed
  segment is held to its manifest, so one cut short at an entry boundary is `Corrupt`
  (`ManifestMismatch`). The restore drill proves both refusals at fixed offsets.

Operator action: none. A store that verified clean before and holds no such damage verifies clean
now. Run `bijective-deploy restore-drill` on the latest backup to confirm.

## 0.1.1

- **An MCP server for research agents** (decision 0020): `bijective-mcp` lets an AI agent write,
  run and judge strategies through the research programs, read-only unless the operator allows
  backtests, with no route to an order ([connect a research agent](docs/how_to/connect_a_research_agent.md)).
- **The artefact carries every program its documentation uses**: the console, the operator command
  line (`bijective`), the keystore tool (`bijective-secrets`), the deployment kit (`bijective-deploy`)
  and `bijective-mcp`, beside the programs 0.1.0 carried.
- **Documentation site** at https://docs.bijectivelabs.dev/bijective/, built from `docs/` in the
  same commit as the source it describes.

Operator action: install the units generated from the 0.1.1 manifest, as for any release
([install a node and bring it up](docs/how_to/install_and_start_a_node.md)). The store and the
configuration carry over unchanged.

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
