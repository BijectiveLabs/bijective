# Check whether a node is up

Platform-Version: 0.1.0
Drill: crates/deployment/tests/unit/tests.rs
Applies-To: any running host

## Steps

`bijective-deploy status <manifest.json> <run-dir>`. It exits 0 when the node is ready and non-zero
otherwise, so a script can ask without parsing prose.

## Reading the answer

| State | Meaning |
|---|---|
| `Down` | No core program has reported. The node has not been started |
| `Starting` | A core program is reporting and not yet ready. The line names which and why |
| `Ready` | Every core program has finished starting. **Trading is a separate state** |
| `Failed` | A core program failed, **or one stopped reporting**. The line says which |

**`Failed` covers two different situations and distinguishes them in the text.** *Stopped reporting*
means a program that was running has gone - look for a crash. *Failed* with a program's own words
means it refused to start - read the words.

**A `DEGRADED` line beside a `Ready` node is not a contradiction.** An operational program is not
working and the core does not need it. What has stopped is the ability to know the record is right,
not the ability to run.
