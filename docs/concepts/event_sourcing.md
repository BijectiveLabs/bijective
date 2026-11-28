# Event Sourcing

Events in the store are the source of truth. Every long-lived program rebuilds its working state by
replaying them at start, and the same input sequence replays to byte-identical output.

## The clock

Every component reads time through `bijective-common::clock`. It has two implementations:

- **Wall clock.** Each reading is the later of the operating system clock and the last reading. A
  time correction that steps the host back holds time still until the host passes it again, so time
  never moves backwards under a running program. A suspended host is caught up on the first reading
  after it resumes: a monotonic clock does not count a suspension, and a clock built on one fell
  behind the venue by the length of every sleep (measured 2026-11-27 on a WSL host: 3923 s, and every
  signed venue request was refused as outside the venue's receive window).
- **Sequence clock.** Driven by the timestamps of the events being replayed. Equal timestamps are
  permitted. A timestamp earlier than the last one is refused, because a replay that silently
  declined to move backwards would produce a different history while reporting success.

Time is nanoseconds since the Unix epoch, held as `i128`. The `Clock access` gate refuses a
system-clock read anywhere outside the clock crate.

## The event store

`bijective-event-store` stores each stream as a directory of segment files.

```
<root>/<stream>/.writer.lock          single writer per stream
<root>/<stream>/000000000000.seg      segment
<root>/<stream>/000000000000.manifest written when the segment is sealed
```

- **Append-only.** A byte that has been written is never rewritten: not on recovery, not on
  rotation, not on repair. There is no repair.
- **Hash-chained.** Each entry's link is computed over the previous link, the sequence position,
  the payload length and the payload. Editing any entry breaks the chain at the next one.
- **Durable before acknowledged.** `append` returns only once the write is durable. A writer that
  makes a batch durable at once uses `append_unsynced` and then `sync`, and acts on nothing in the
  batch until `sync` returns. Only the market journal does this
  ([decision 0019](../decisions/0019-market-journal-durable-per-batch.md)). Control, order and
  limit events keep one sync per decision.
- **Single writer.** A stream has one writer, enforced by a lock file. A stale lock left by a crash
  is cleared by an operator, never broken automatically: two writers on one stream would put two
  entries at one position, which no later process can repair.
- **Sealed segments are read-only.** Sealing writes a manifest and removes write permission.
- **Retention removes a prefix, never a middle.** `retention::prune_before` deletes the oldest
  sealed segments, never the newest. It first writes `<root>/<stream>/horizon`: the first position
  the stream still holds and the link that position extends, with a digest over both. `read_range`
  refuses a range starting below the horizon (`Pruned`), so a shortened stream is never read as a
  whole one, and an edited horizon is refused (`HorizonUnreadable`). Only the frame journal is
  pruned. The control and proposal streams are kept whole.
- **A polling reader reads on from a cursor.** `cursor::read_after` returns what a stream gained
  since a `Cursor` (segment, byte offset, position and chain link where the last read ended),
  reading and verifying only those bytes, chained from that link. It relies on the two rules
  above: bytes before the cursor are never rewritten. A segment found shorter than the cursor
  was replaced and is refused (`Rewritten`). Damage after the cursor stops the read as it does
  for `read_range`.

Verification returns one of three verdicts:

| Verdict | Meaning |
|---|---|
| `Clean` | Every complete entry chains correctly. An empty segment is clean with zero entries |
| `Corrupt` | An entry does not match the chain, the sequence broke, a length is out of range or damaged, or a sealed segment does not hold what its manifest declares. The segment is quarantined, never repaired |
| `Unreadable` | Not a segment: truncated header, wrong magic, unknown format version, or an I/O failure |

A process killed in the middle of an append leaves a torn tail. Those bytes were never a record,
so the segment reports `Clean` with `incomplete_tail_bytes` set. Recovery seals the segment at its
last whole entry, leaves the tail on disk as it was found, and opens a new segment.

Shape alone does not tell a torn tail from damage. A length field damaged to claim more than the
segment holds runs past the end exactly as an interrupted write does, and read as a torn tail it
would drop every acknowledged entry after it. The verifier therefore asks the entry itself: a torn
write leaves only a prefix of its payload, so its link verifies at no end. When the link verifies at
the end of the segment, or where a header for the next position begins, the entry was whole and the
segment is `Corrupt` (`PayloadLengthDamaged`). A sealed segment is also held to its manifest: entry
count, first position, sealed length and last link must match, so a sealed segment cut short at an
entry boundary is `Corrupt` (`ManifestMismatch`), not a shorter clean one. A manifest that is
missing or does not verify leaves the judgement to the bytes, so an interrupted seal never makes a
stream unopenable.

## Encoding

`bijective-serialization::codec` defines a versioned, fixed-layout binary encoding:

- Every payload starts with a four-byte envelope: class id and class version.
- The writer has no method that accepts a floating-point value, so a payload cannot carry one.
- Field order is the layout. There are no maps and therefore no iteration order to canonicalise.
- Every length is bounds-checked before it is used, and trailing bytes are an error.
- An unknown class, or a version newer than the decoder knows, refuses the replay. It is never
  skipped: skipping one limit-consumed event would silently restore spent budget.
- Class ids are allocated in one place and never reused once written to a store.

## Projections

`bijective-event-store::projection` implements rebuild-by-replay once, for every program:

- A projection that has not been rebuilt cannot be read. `Recovered::state` returns `None` until the
  replay has completed over the whole stream.
- An entry that cannot be decoded, or that the projection rejects, refuses the whole rebuild and
  reports its position.
- Advancing with live events before recovery is refused.
- An empty stream is a verified fact (`Verified { through: None }`), distinct from a stream that was
  not read.

There are no snapshots or checkpoints yet. Each program holds its own projection. None is shared.

## The bus

`bijective-common::bus` carries messages between components inside one program.

- **Single writer per topic, by type.** `claim` returns a `TopicWriter` and a topic can be claimed
  once, so a second writer cannot exist.
- **Acknowledged subscriptions.** `subscribe` returns a `Subscription` carrying when it attached.
  There is no way to hold a subscription the bus did not register.
- **Refusals are returned, not logged.** When a subscriber's bounded queue is full, publication
  reports which subscription refused and how far behind it was. Each subscriber states its own
  capacity. There are no unbounded queues.
- **Deterministic delivery.** Subscribers are visited in subscription order.

## Determinism

`bijective-determinism` drives an input sequence through the clock, the bus and the store together
and returns the bytes the store holds. Its tests require that:

- two runs of the same sequence are byte-identical, and the output is not empty,
- changing one payload, or one timestamp, changes the output,
- a refusal stops the run rather than producing a shorter history.

The `Determinism` and `Replay parity` gates run these checks on every change.
