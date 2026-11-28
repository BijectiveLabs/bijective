# Installation

## Requirements

| Requirement | Version | Why |
|---|---|---|
| Linux | cgroup v2 | Strategy runs are bounded with cgroup v2 and Linux namespaces |
| systemd | 250 or newer | Encrypted init credentials for the core programs; a delegated subtree for the strategy engine |
| Rust | 1.97.1, pinned in `rust-toolchain.toml` | The workspace's minimum supported version |
| Python | 3.12 to 3.14 | The strategy library and the read-side packages |
| `unshare(1)` | util-linux | Entering the sandbox namespaces |

No database, cache or message broker is required. The event store is the durable state.

Building and running the unit tests works on other platforms; the sandbox and the drills in
`scripts/verify/` need Linux.

## On your computer

Every run of a strategy is bounded by the Linux kernel: its own cgroup for memory and process
limits, an empty network namespace, no credentials. Windows and macOS have neither, so the platform
runs inside a Linux on them, the way container and cluster tools do. On Windows or macOS,
`bijective-backtest` and `bijective-paper` refuse to start and name this fix
([decision 0022](../decisions/0022-a-linux-host-on-every-computer.md)).

| Computer | Linux to run it in | Status |
|---|---|---|
| Linux with systemd 250 or newer | the host itself | Measured |
| Windows 10 or 11 | WSL 2: `wsl --install -d Ubuntu` in a Windows terminal, then open Ubuntu | Measured on Windows 11 in WSL 2 Ubuntu with systemd (2026-11-28) |
| macOS | a virtual machine running Ubuntu 24.04 or newer with systemd | Not yet measured by this project |

On Windows, check that the Ubuntu runs systemd: `ps -p 1 -o comm=` answers `systemd`. Microsoft
documents systemd as the default for the current Ubuntu that `wsl --install` installs; another
distribution, or an older one, enables it as
[Run a node on a Windows workstation](../how_to/run_a_node_on_a_windows_workstation.md) step 1
describes.

Inside that Linux, the release artefact is the shortest start: no Rust toolchain and no build.
Ubuntu ships `python3`, which strategies need.

```bash
v=0.1.1
base=https://github.com/BijectiveLabs/bijective/releases/download/v$v
curl -fsSLO $base/bijective-$v-linux-x86_64.tar.gz
curl -fsSLO $base/bijective-$v-linux-x86_64.tar.gz.sha256
sha256sum -c bijective-$v-linux-x86_64.tar.gz.sha256
tar xzf bijective-$v-linux-x86_64.tar.gz
export PATH="$PWD/bijective-$v-linux-x86_64/bin:$PATH"
```

Then follow the [Quickstart](quickstart.md). Measured 2026-11-28 in WSL 2 on Windows 11 with the
0.1.1 artefact: `bijective-backtest --new alpha.py` and `bijective-backtest alpha.py` completed in
1.6 seconds, each run bounded in a scope the command made itself. The artefact is built for
x86_64. A machine with an Arm processor, such as a Mac with Apple silicon, builds from source
below until an Arm artefact is released.

## Build

```bash
git clone https://github.com/BijectiveLabs/bijective.git
cd bijective
cargo build --release
```

The binaries are in `target/release/`.

## Python packages

```bash
python3 -m pip install -e "python/bijective-strategy[test]"
python3 -m pip install -e "python/bijective-attribution[test]"
python3 -m pip install -e "python/bijective-detection[test]"
```

`bijective-strategy` has no dependencies. A Python strategy run in the sandbox imports it from
`lib/python/` beside the binaries, or from `python/bijective-strategy/src` in a source checkout.

## Verify the build

```bash
cargo test --workspace
bash scripts/ci/run_gates.sh
```

The gate runner reports every gate, the subjects each examined, and fails if any gate failed or
could not run. See [CI gates](../developer_guide/ci_gates.md).
