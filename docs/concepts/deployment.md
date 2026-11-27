# Deployment

The platform is self-hosted. The operator deploys, configures and operates it, and keeps custody of
keys and data.

## One manifest

`crates/deployment/deployment.json` declares everything a node runs: each program's binary,
arguments, start order, memory bound, stop timeout, the paths it may write and the credentials it
receives, plus scheduled checks such as venue assurance.

`bijective-deploy` turns the manifest into init-system units and checks a release:

| Command | Purpose |
|---|---|
| `generate <manifest> <unit-dir>` | Writes units and prints which to enable and which to install stopped. It changes nothing on the host |
| `verify-units <manifest> <unit-dir>` | Regenerates and diffs, so a hand-edited unit is detected |
| `record`, `verify-artefact` | Records a release artefact and verifies it before it is unpacked |
| `status <manifest> <run-dir>` | One readiness answer for the whole node |
| `restore-drill` | Verifies a restore from backup |
| `notices` | Generates the third-party notices |

Unit generation is deterministic: no timestamp, no host name, no map iteration order, so the same
manifest produces the same bytes.

## Program classes

- **Core** programs (data engine, control engine) are enabled and started.
- **Operational** programs are installed stopped. The core starts, ingests and runs without them.
- The **strategy engine** is installed and does not start on boot. Which strategy runs, on which
  market and at which stage is the operator's choice and is given as unit environment:

| Variable | Meaning |
|---|---|
| `BIJECTIVE_CGROUP_ROOT` | The delegated cgroup v2 subtree runs are bounded in |
| `BIJECTIVE_STRATEGY` | The strategy program |
| `BIJECTIVE_MARKET` | The market stream it reads |
| `BIJECTIVE_STAGE` | `paper` or `live`; never left unset |

## Containers

`bijective-deploy compose <manifest>` prints a container composition from the same manifest, so the
units and the composition cannot describe different nodes. Each program is a service running its
manifest binary from one image (`Dockerfile` at the repository root). What the units state is
stated again: a read-only root, no new privileges, no capability, the memory bound, the stop
timeout and the start order. A path a program writes is a named volume shared by the programs that
declare it, and a credential is a file secret mounted where the broker reads it
(`CREDENTIALS_DIRECTORY=/run/secrets`). The core services start by default; the operational
programs need `--profile operational`. Checks are not services: schedule them on the host.

```text
bijective-deploy compose crates/deployment/deployment.json > compose.yml
docker compose -f compose.yml up
```

Measured 2026-11-27 on Docker Desktop (Windows 11, WSL 2 backend): the image built from the
`Dockerfile`, and the data engine service from a composition of a node manifest, with its venue
profile supplied by an override as a read-only bind mount, reported `ready` on the Bybit public
linear stream under a read-only root with every capability dropped. The image carries the system
trust roots (`ca-certificates`): the venue sockets verify TLS against them, and an image without
them reached no venue while its readiness said only `link Disconnected`.

Not yet covered, and stated so a first run is not a surprise:

- The console binds loopback inside its container, which the host cannot reach, and it refuses
  any other address without HTTPS and client certificates
  ([Reach the console](../how_to/reach_the_console.md)). Run it on the host against the store, or
  give it the three certificate files.

- The composition mounts what a program writes, not what it reads. The configuration, the venue
  profiles and the keystore (`/etc/bijective`, `/var/lib/bijective/keystore.bjks`) are supplied by
  an override file as read-only bind mounts.
- The strategy engine bounds each run in a delegated cgroup v2 subtree. A container is not given
  one, and no run has shown that it works there; run the strategy engine under the init system
  until it has.
- The image holds no Python runtime, so it cannot run client strategies.
- The change gates do not build the `Dockerfile`. The release workflow
  (`.github/workflows/release.yml`, on demand) builds the image and keeps it as a workflow artefact.
  Nothing is published. The composition is checked against the manifest by
  `crates/deployment/tests/shipped_compose.rs`.

## Operators

The control engine in live mode reads `/etc/bijective/operators.json`, the registry of who may sign
commands (see "Who is asking" in [Risk and limits](risk_and_limits.md)). Create a key with
`bijective keygen <name> <role> <directory>`, add the printed entry to the file, and keep the
private key off the node: the node holds public keys only.

## Ready is not trading

`bijective-deploy status` reports `Ready` when every core program has finished starting. The trading
state still rests at `Halted` until an authorised operator resumes it. Readiness is read from the
programs' readiness documents, never from the init system: a process that is alive and still
replaying is not ready.

## Host requirements

- Linux with systemd 250 or newer for encrypted init credentials.
- cgroup v2 with a subtree delegated to the strategy engine's unit (`Delegate=yes`, emitted by the
  generator).
- No database or cache server is required. The event store is the durable state.
- A Windows 11 workstation qualifies through WSL 2 with systemd enabled, until a server is
  available: [Run a node on a Windows workstation](../how_to/run_a_node_on_a_windows_workstation.md).

## Venue profiles

Each venue has a profile at `/etc/bijective/profiles/<venue>.json`, which the scheduled venue check
verifies against. `crates/deployment/profiles/example.json` is a complete, valid profile for a
venue that does not exist. Four format rules a first attempt usually gets wrong:

- Wide integers travel as strings: `"price_tick": "10"`, not `10`.
- Enumerations are tagged and hyphenated: `{"rule": "increment-only"}`,
  `{"model": "published", "interval_ns": "28800000000000"}`.
- The instrument catalogue is a plain list; the symbol is a field of each entry.
- The support entry names the same venue as the profile.

A profile carries no verification date. Whether it has been verified is recorded beside it, on the
assurance stream.

## Procedures

The [how-to guides](../how_to/index.md) cover verifying a release, installing and starting a node,
checking a node, restoring from backup, recovering an interrupted start, loading a credential and
moving a node to another host. Each guide names the drill in `scripts/` that exercises it.
