# Technology

Each choice below is fixed by a measurable requirement: latency on the order path, byte-identical
replay, licence compatibility, and the number of services an operator must run.

| Area | Choice | Requirement it meets |
|---|---|---|
| Language | Rust 1.97.1 (edition 2024) for the core, engines and adapters, Python 3.12+ for strategies and read-side tooling | Bounded worst-case latency on the order path, `unsafe_code = "forbid"` in every crate |
| Event store | Embedded append-only segment files on the node's filesystem, SHA-256 hash chain, standard library only | One durable write per decision, no database server to operate |
| Market journal durability | One sync per batch of frames already waiting, journal before derived streams ([decision 0019](../decisions/0019-market-journal-durable-per-batch.md)) | One instrument's recorded Bybit session replayed at 4,370 frames/s to disk against 163 with a sync per frame, and 65 frames/s as recorded (WSL 2 host, 2026-11-27) |
| Research archive | Apache Parquet (`parquet` crate, column API, arrow features off) with zstd level 3, one file per minute of received time, a JSON-lines manifest with SHA-256 per file | Columnar files every quant tool reads, a projection of the frame journal, rebuilt byte for byte |
| Event encoding | Versioned fixed-layout binary: 4-byte envelope (class id, class version), big-endian fixed-width fields, no floating point | Byte-identical replay, an unknown class or newer version refuses the replay |
| In-process messaging | Bounded channel per topic owned by the program, single writer per topic | Back-pressure is reported to the publisher, nothing is dropped silently |
| Cross-process messaging | The event store, no message broker | A message lost between processes cannot be told from one never sent |
| Durable state | The event store only, projections are in memory per process and rebuilt by replay | No cache or database client in any component's required dependencies |
| Relational store | Optional, behind a port (PostgreSQL store available for attribution) | Operators may keep reporting in the database they already run |
| Pre-trade check | Synchronous evaluation on the order path's thread, state owned by value, no lock, no queue, no allocation per call | Worst case linear in the number of declared limits |
| Limit state | Declarations, consumption and releases as events, consumed budget is a replay | A restart cannot restore spent budget |
| Strategy sandbox | Linux namespaces and cgroup v2, driven by the strategy engine, no container runtime | No network, bounded memory and time, no credentials inside a run |
| Console | One Rust binary rendering HTML server-side on loopback or HTTPS with client certificates, assets embedded | A failed read shows as failed, not as the last good value |
| Installation | Units for the distribution's init system, generated from one manifest | A stopped program is a state the operator can produce and test |
| Secrets | Encrypted keystore file (ChaCha20-Poly1305), key from a declared source, no fallback between sources | Credentials never in configuration, environment or source |
| Configuration and profiles | JSON, with wide integers written as strings | Values survive parsers that read JSON numbers as doubles |
| Venue websocket | `sockudo-ws` behind one shared crate (`bijective-venue-socket`), TLS from the host's roots, permessage-deflate offered on every connection (RFC 7692) | A venue stream fits a narrow link: one instrument's book, trades and ticker measured 0.4 to 0.9 s behind the venue with compression, up to 106 s without it on the same host (2026-11-24 and 2026-11-26) |
| TLS | `rustls` with the `ring` provider, one provider per binary | One audit surface, no C toolchain dependency for TLS (the archive's zstd library is the one C component, built from its bundled source) |
| Hyperliquid signing | ECDSA on secp256k1 (`k256`), Keccak-256 (`sha3`), msgpack written field by field (`rmp` primitives) | A field reorder cannot change a signature unnoticed |
| Order cost | Cost charged on submission, released on a terminal outcome | A limit cannot be spent twice by an order that is still open |
| Venue identity | Venue symbols are translated outward only, above the adapter contract instruments are `InstrumentId` | Two venues write into one store without pricing each other's positions |

## Measured and not adopted

A technology is adopted when a measurement on this platform shows it meets a need the platform has.
Each one below was examined against that test. The reason is recorded with its evidence, and with
what would make the question worth asking again.

| Technology | Why not | Evidence | Asked again when |
|---|---|---|---|
| Zero-copy parsing | The flush to disk, not copying, bounds the market path | A durable append cost 2.1 to 2.5 ms against a few nanoseconds per copy (2026-11-12). Removing the flush made the recorder 32 times faster with every copy still in place: 5,150 against 163 frames/s (2026-11-27) | A production host approaches the processing bound, 5,150 frames/s for one instrument on the measured host |
| Redis or another external cache | A second service on the capital path, with state that is lost on restart and has to be reconciled again | The event store is the durable state and every projection is rebuilt by replay (`Determinism` and `Replay parity` gates). The `No required datastore` gate keeps every component free of a cache client | A second consumer needs state shared across hosts that a replay cannot rebuild in time |
| A message broker | A message lost between processes could not be told from one never sent | Programs on a node exchange records through the event store, whose hash chain and sequence numbers make a gap visible | Programs of one node run on more than one host |
| A required database server | An operator would have to run, back up and secure it before the first trade | PostgreSQL is available behind a port for attribution reporting, and no component requires it (`No required datastore` gate) | Never as a requirement. Reporting stays optional |
| Python loaded into the engine (PyO3) on the order path | Client strategy code would share the process that holds credentials and sends orders | Strategies run in a sandbox of their own: no network, bounded memory and time, no credentials (decision 0003, `sandbox.sh` drill). The research archive is Parquet, which pandas, Polars and DuckDB read without a binding | A read-only research binding is requested by a second user. The order path stays out of reach |
| A container runtime for the strategy sandbox | One more runtime between the supervisor and the kernel controls it already uses | Linux namespaces and a delegated cgroup v2 subtree bound every run (decision 0003) | A host cannot delegate a cgroup subtree |
| A timer for batching market writes | It delays every frame in a quiet market to gain throughput only in a busy one | Batching what is already waiting reaches 4,370 frames/s with no wait added (decision 0019) | Never: the drained batch already reaches the throughput a timer would |

## Dependency policy

Most core crates depend on the standard library only. Third-party packages are limited to
networking, TLS, cryptography, serialization and the research archive's file format. Every package in `Cargo.lock` is attested with its
licence in `scripts/ci/licence_attestation.tsv`, and only permissive licences are accepted. The
`Dependency licences` and `Licence linkage` gates enforce this.
