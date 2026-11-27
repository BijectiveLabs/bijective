# Developer Guide

- [Environment setup](environment_setup.md): toolchains, building, and running the drills on Linux.
- [Quality gates](ci_gates.md): the checks every change passes before it merges.

How to propose a change is described in [CONTRIBUTING.md](../../CONTRIBUTING.md).

## Where tests live

Test code and source code are in separate folders.

| Kind | Location | Sees |
|---|---|---|
| Rust unit tests | `crates/<crate>/tests/unit/`, mirroring the `src/` path of the module they test | The module's private items: each file is declared from its module as `#[cfg(test)] #[path = "../tests/unit/<file>.rs"] mod tests;` |
| Rust integration tests | `crates/<crate>/tests/*.rs` | The crate's public interface only; each file is its own test target |
| Python tests | `python/<package>/tests/` | The package as installed |
| Fixtures | `crates/<crate>/fixtures/` | Read by tests through relative paths |

A test file, test directory or inline test module under `src/` is refused by the `Test layout` gate.
