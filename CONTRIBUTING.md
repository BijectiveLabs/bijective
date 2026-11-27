# Contributing

A contribution is accepted under the Apache License 2.0, the licence of this repository
(section 5 of the licence). There is no separate agreement to sign.

## Branches

| Branch | Purpose |
|---|---|
| `master` | Released code. Changes arrive by reviewed pull request with passing CI |
| `dev` | Active development. Pull requests target this branch |

## Commit messages

```
<type>(<scope>): <concise description>
```

`type` is one of `feat`, `fix`, `refactor`, `test`, `docs`, `chore` or `perf`.

```
fix(execution): keep the budget reservation when a submit times out
feat(adapter/okx): add the public order book stream
```

## Before opening a pull request

```bash
cargo test --workspace
for pkg in python/*/; do (cd "$pkg" && python3 -m pytest -q); done
bash scripts/ci/run_gates.sh
```

All three must pass. See [Environment setup](docs/developer_guide/environment_setup.md) for the
toolchains and [Quality gates](docs/developer_guide/ci_gates.md) for what the gate runner checks.

A pull request:

- includes a test that fails without the change and passes with it;
- updates, in the same pull request, every document that describes the changed behaviour, flag,
  format, stream, default or command;
- states, for a change to the order path, risk limits or stored state, what happens to an open
  order, to reserved budget and to the trading state if the changed component fails;
- keeps source files under 600 lines and functions under 80 lines, formatted by `cargo fmt`
  with the repository's `rustfmt.toml`;
- keeps formatting changes and logic changes in separate commits;
- starts every new `.rs`, `.py` and `.sh` file with the licence notice the other files carry
  (`SPDX-License-Identifier: Apache-2.0` and the copyright line), after a shebang where there is
  one. A contributor may add their own copyright line below it; existing notices are not removed.

## Names

The licence covers the code, not the name (section 6 of the licence). See
[TRADEMARKS.md](TRADEMARKS.md) for how the Bijective name may be used by a fork or a
redistribution.

## Security

Do not report vulnerabilities in issues or pull requests. See [SECURITY.md](SECURITY.md).
