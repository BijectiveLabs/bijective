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
