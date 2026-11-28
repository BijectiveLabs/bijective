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

- **The supervisor refuses a subtree it cannot move a run into.** Its probe wrote nothing to
  `cgroup.procs`, which passes wherever the file opens. Moving a process also needs write access
  to the common ancestor, which a `su` or `ssh` session under systemd lacks, and every run then
  ended with exit 111 and no reason. The probe now moves a process in, as a run is moved.

- **Release provenance.** The release workflow signs build provenance for the archive and the
  strategy library before it publishes them, checked with `gh attestation verify`
  ([verify a release](docs/how_to/verify_a_release.md)). Issue forms and a code owners file are
  added to the repository.

- **Dependencies with published vulnerabilities are updated**, and a new gate, `Dependency
  advisories`, refuses a lock that holds one. `rustls` 0.23.46, `rustls-webpki` 0.103.17,
  `webpki-root-certs` 1.0.12, `url` 2.5.9, `ctutils` 0.4.8, `tokio` 1.54.1, `parquet` 60.2.0, and
  `k256` 0.14 for Hyperliquid and Polymarket signing, which leaves `crypto-bigint` 0.5 and its
  timing advisory. Signatures are byte for byte the same: the Polymarket reference vectors pass.

- **Event store format 2** (decision 0021): each entry header carries a check over its marker,
  length and position, so a damaged length is refused (`HeaderCheck`) wherever it lies, with or
  without its payload. Format 1 segments are still read. A stream moves to format 2 at its next
  segment: the active format 1 segment is sealed as it stands.

- **A first command on Windows or macOS names the fix.** `bijective-backtest` and `bijective-paper`
  refuse to start on a computer without Linux cgroup v2 and say how to get a Linux there (WSL 2 on
  Windows), instead of failing later on a symptom such as a missing `python3`
  ([installation](docs/getting_started/installation.md#on-your-computer), decision 0022).

Operator action: **an upgraded store cannot be opened by 0.1.1 or earlier**, which refuses a
format 2 segment as an unknown version. Back up the store before upgrading if a downgrade may be
needed. Otherwise none. A store that verified clean before and holds no such damage verifies clean
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
