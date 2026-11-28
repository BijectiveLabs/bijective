# Tests

The unit tests of `bijective-decision-capture`. Each file in `unit/` is compiled into the module it tests, through a
`#[cfg(test)] #[path = ...]` line in that module, so a test reaches private items without making
them public. Tests live here, never under `src/` (the `Test layout` gate).

```bash
cargo test -p bijective-decision-capture
```
