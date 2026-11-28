# Detect a unit that no longer matches the release

Platform-Version: 0.1.1
Drill: crates/deployment/tests/unit/tests.rs
Applies-To: any running host; run after any change to the init configuration

## Why

A hand-edited unit runs something the release did not test, and **the difference is invisible until
a restart** - which is the worst moment to discover it. The generator's output is reproducible from
the manifest, so a diff is a check rather than an opinion.

## Steps

1. `bijective-deploy verify-units <manifest.json> <unit-dir>`.
2. A pass reports how many units match.
3. `DRIFT` names the unit that differs. `MISSING` names one that is absent.

## If it reports drift

Do not edit the unit back by hand - that reproduces the same class of problem. Change the
**manifest**, regenerate, and reinstall. If the manifest cannot express what the host needs, that is
a finding about the manifest, and it is recorded as one.
