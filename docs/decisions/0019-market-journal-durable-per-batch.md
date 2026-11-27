# 0019: The market journal is made durable per batch

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-27. Built |
| Extends | The market archive's order of writes (`docs/concepts/market_archive.md`), the event store (`docs/concepts/event_sourcing.md`) |

## Context

The data engine synced every journal entry, and every top and fact derived from it, one at a time:
up to three flushes per frame. The flush, not the processing, set what one venue connection can
take. Measured on 2026-11-27 by replaying a recorded Bybit linear session (one instrument, 496,461
frames) through the recorder on a WSL 2 host:

| Store on | Frames per second | What bounds it |
|---|---|---|
| The host's disk, one flush per entry | 163 | The flush. The process waited on I/O at 6.5 % CPU |
| Memory (tmpfs, where a flush costs nothing) | 5,150 | Parsing and encoding |
| The live session it was recorded from | 65 on average | The venue |

A ceiling of 2.5 times the average of one instrument is crossed by an ordinary burst, and a mass
liquidation multiplies message rates. A data engine that falls behind makes valuations stale at the
moment they matter most. Copying is not the cost: removing the flush makes the same path 32 times
faster, and no copy was removed.

## Decision

1. **A batch is what is already waiting, never what might arrive.** A live ingest applies the event
   it received and whatever is already in the adapter's channel, up to 256 events, then commits.
   Nothing waits for more events, so an idle stream still commits every frame and its latency does
   not change. A replay commits every 256 entries.
2. **A commit keeps the order of the writes, not only of the flushes.** The batch's journal entries
   are written as they come and flushed once. Only then are the tops and facts derived from them
   written, and flushed once per stream. A reader sees an entry as soon as it is written, so a
   derived entry written before its frame's flush could be acted on and then lost with the frame.
3. **Only the market journal and its derived streams.** Control, order and limit events keep one
   flush per decision. The event store gains `append_unsynced` and `sync` beside `append`, and
   `append` is the two together.
4. **A rotation flushes first.** A sealed segment's manifest describes entries on disk.

## Consequences

- Measured on the same host and corpus: **4,370 frames per second to disk**, 27 times the per-entry
  rate and within 15 % of the processing bound. One instrument's average now uses 1.5 % of it.
- A crash loses at most the batch that was not yet flushed: frames still in the channel were never
  acted on, and no top or fact from them was written. Before, the bound was one frame. The journal's
  torn-tail recovery is unchanged.
- The journal, the market stream and the archive hold exactly what they held before, in the same
  order. Replay and the archive rebuild are byte-identical as before (`Market archive` and
  `Replay parity` gates).
- The figures are this host's. A server disk flushes faster, so the per-entry ceiling there is higher,
  and the batch multiplies whatever it is.

## Rejected

- **A timer-based batch** (flush every few milliseconds) adds that latency to every frame in quiet
  markets to gain throughput only in busy ones. Draining what is already waiting gains the same
  throughput with no added latency.
- **Not flushing the market journal at all.** It would remove the record-before-act guarantee the
  archive and every replay rest on.
- **Zero-copy parsing.** The measurement shows copying is not what bounds the path.
