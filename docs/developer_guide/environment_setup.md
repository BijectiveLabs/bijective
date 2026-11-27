# Environment Setup

## Toolchains

- Rust 1.97.1, pinned in `rust-toolchain.toml`. `rustup` installs it on first use in the
  repository.
- Python 3.12 to 3.14.
- `bash`, `shellcheck` and `shfmt` for the scripts.

```bash
cargo build --workspace
cargo test --workspace
for pkg in python/*/; do python3 -m pip install -e "${pkg}[test]"; done
bash scripts/ci/run_gates.sh
bash scripts/ci/hooks/install.sh
```

## Drills that need Linux

The sandbox tests and several drills in `scripts/verify/` bound real processes and need cgroup v2
with a subtree delegated to the user running them. The supervisor must already be inside that
subtree. On a systemd host:

```bash
systemd-run --user --scope \
  --setenv=BIJECTIVE_CGROUP_ROOT=/sys/fs/cgroup/<delegated-subtree> \
  -- ./scripts/verify/sandbox.sh
```

A drill refuses rather than guesses a cgroup root, and it cannot pass against a directory that is
not one. See [Verify an agent run can be bounded](../how_to/verify_an_agent_run_can_be_bounded.md).

## Offline builds

The gates build with `--offline` and resolve the workspace against the local package registry. Run
`cargo fetch` once with network access before running the gates on a new machine.
