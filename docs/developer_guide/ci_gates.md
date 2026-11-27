# Quality Gates

Every change runs the gates in `scripts/ci/run_gates.sh`, locally through the pre-push hook and in
CI on every push and pull request to `master` and `dev`. A failing gate blocks the change.

## Rules every gate follows

- A gate that cannot run is reported as a failure, never as a skip. An unverified change is
  unverified, and a silent skip would pass exactly the case the gate exists to catch.
- A gate reports how many subjects it examined, and a pass over none is a failure. A search that
  covered nothing returns the same empty result as a search that found nothing wrong.
- Subjects are discovered, not listed. A gate that enumerates its own subjects skips whatever is
  added next.

## Running the gates

```bash
bash scripts/ci/run_gates.sh
bash scripts/ci/hooks/install.sh   # run them before every push
```

Several drills need a Linux host with a delegated cgroup v2 subtree; see
[Environment setup](environment_setup.md).

## The gates

| Gate | What it enforces |
|---|---|
| **Tests** | Every crate and Python package passes its tests |
| **Boundary** | The core starts, ingests and runs with the optional programs stopped |
| **Safe interruption** | An interrupted start leaves the platform halted, and a stale writer lock is refused rather than broken |
| **Recovery drill** | A restore is verified, and both of its refusals are demonstrated |
| **Keystore drill** | A credential loads through the secrets broker and the seal holds |
| **Clock access** | Wall time is read in one place only, the clock module of `bijective-common` |
| **No required datastore** | No component requires a cache or database client; a relational store stays optional |
| **Dependency direction** | Every internal dependency follows `scripts/ci/areas.txt`: an engine reaches only core, adapters and its own area; no open area reaches a commercial component |
| **Order path closed** | Submitting an order requires a clearance, and a clearance is minted only by a risk evaluation in the control engine |
| **Determinism** | One input sequence replayed twice produces byte-identical output |
| **Runbooks** | Every runbook is bound to the platform version and to a drill that exists |
| **Licence linkage** | No copyleft-only package in the resolved dependency tree; an unresolvable tree fails |
| **Security checks** | No secret literals, and no network egress above the adapter contract |
| **Credential resolution** | No credential is resolved from configuration or the environment |
| **Pre-trade check bound** | No dependency, I/O, lock, wall clock or allocation on the risk decision path |
| **Structure** | Every component is declared in exactly one architectural area |
| **No floating point** | No floating-point type in non-test source; prices and quantities are scaled integers |
| **Venue names** | No venue is named above the adapter contract |
| **Order attribution** | No adapter request carries a broker id, referral, channel code or builder fee, and every request header is on the allow-list |
| **Dependency licences** | Every third-party package in the lock is attested and on the allow-list |
| **Third-party notices** | `THIRD_PARTY_LICENSES.md` matches what the attestation generates |
| **Source notices** | Every tracked `.rs`, `.py` and `.sh` file outside `pro/` carries the SPDX licence line and the copyright notice in its first four lines |
| **Pro boundary** | No file of the open core names `pro/` (only the workspace manifest, its lock and `areas.txt` list a Bijective Pro crate, so every gate judges it; the export removes those lines), so the public export drops it whole, and every commercial source carries the proprietary notice and no Apache-2.0 line |
| **Agent environment** | No strategy crate reaches a credential or a venue, and the sandbox's no-network test is paired with a control run that must reach the network |
| **Venue references** | Every venue adapter cites the venue documents it was built from, with the date each was read |
| **Market archive** | Frames are journaled before they are read, the archive verifies against its manifest, a rebuild from the journal is byte-identical, a killed run's journal rebuilds a sound archive, a stop request ends the program cleanly, compaction is proven equal, retention removes only journal segments a verified archive holds and refuses an archive from another journal, and peak memory stays within its declared bound |
| **Console TLS** | Over HTTPS the console refuses a connection without a client certificate its authority issued and does not answer plain http; a certified client reads the control page, a form carrying the page token is signed into the command stream, and a form without it is refused and appends nothing |
| **Venue isolation** | Two venues run side by side, each in its own stream, neither carrying the other's identifiers |
| **Unread declarations** | Every public item is read somewhere, or listed with a reason in `scripts/ci/unread_declarations.txt` |
| **Comment budget** | No comment block longer than eight lines |
| **Comment punctuation** | No new comment line with a semicolon (CLAUDE.md 2.8): the count in `scripts/ci/comment_semicolons.txt` may fall, never rise, and is lowered with each cleanup |
| **Commit attribution** | No commit carries a co-author trailer or a tool credit (CLAUDE.md 2.1): a commit names its authorised identity only. A shallow CI checkout reads the pushed tip, a local run the whole branch |
| **Test layout** | No test file, test directory or inline test module under any `src/`; tests live in `tests/` beside it |
| **Unit size** | No Rust source file exceeds 600 lines and no function exceeds 80 lines; test sources are exempt |
| **Adapter conformance** | Each adapter that implements `MarketFeed` or `VenueExecution` runs `check_feed` or `check_execution` in its own tests, or names the gap in `adapter_conformance_gaps.txt`; a gap that has been closed must be removed |
| **Order path** | A proposal is answered exactly once, a restart does not answer it again, and the running loop checks that a halt was honoured |
| **Replay parity** | A program's output stream is a function of its clock and its store, and of nothing the host decided |
| **Proposal streams** | The stream the control engine reads is written only by a live strategy session |
| **Release path** | A release artefact can be assembled, recorded and verified, and is refused when a byte changes, a file is added or a listed file is missing |
| **Gate inventory** | This table lists exactly the gates the runner executes |
| **Format and lint** | `cargo fmt --check`, `clippy -D warnings` and pyflakes are clean on every crate and package |
