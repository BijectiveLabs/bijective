# Market Archive

The data engine keeps two records of what a venue sent. The frame journal is the durable record:
every frame exactly as received, written and synced before it is read. The research archive is a
projection of the journal: normalised market records as Parquet files that research and analysis
tools read directly. The archive can be deleted and rebuilt from the journal, and the rebuild is
byte-identical, for as long as the journal has not been pruned (see Retention).

## Order of writes

For each batch, in this order ([decision 0019](../decisions/0019-market-journal-durable-per-batch.md)):

1. Each frame and its arrival time are appended to the `frames-<venue>` stream.
2. The venue feed parses it into market records and, where a book has both sides, a top of book.
   The records are buffered for the archive, and the facts among them and the top are held.
3. The journal is synced. Only then are the held facts appended to `facts-<venue>` and the tops to
   `market-<venue>`, and each of those streams is synced once.

A batch is what is already waiting in the adapter's channel when an event arrives, up to 256
events. Nothing waits for more, so a quiet stream commits every frame. Nothing downstream ever
holds data the journal does not: a reader sees an entry once it is written, and no fact or top is
written before its frame is durable. A process killed at any point loses at most frames of the
batch not yet synced, none of which had been acted on.

The journal also records the events that change how later frames are read: a session starting, a
session ending, and continuity lost (books dropped). Replaying the journal therefore drops books at
the same entries a live run did, and reproduces every record, top and archive row.

## Layout

```
<archive>/<venue>/
  manifest.jsonl                                 one line per event, appended and synced
  book/date=YYYY-MM-DD/seg-<window>-<entry>.parquet
  facts/date=YYYY-MM-DD/seg-<window>-<entry>.parquet
  gaps/date=YYYY-MM-DD/seg-<window>-<entry>.parquet
  book/date=YYYY-MM-DD/compacted-<window>-<entry>.parquet
  quarantine/                                    unlisted files set aside on open
```

A segment holds one minute of received time (`<window>` is its start in UTC nanoseconds) and at
most 100,000 rows. `<entry>` is the journal position of its first row. Both bounds depend only on
the records, so a rebuild seals the same files. The date partition is the window's UTC date.

## Tables

`book` holds one row per level of a book record. A record with no levels (an empty snapshot) is
one row with the level columns absent, because an empty snapshot clears the book.

| Column | Type | Content |
|---|---|---|
| `journal_seq` | uint64 | Journal position of the frame the record came from |
| `record_in_frame` | uint32 | Which record of that frame |
| `received_at` | timestamp (ns, UTC) | Arrival on the platform clock |
| `exchange_time` | timestamp (ns, UTC), optional | The venue's stamp, absent when it publishes none |
| `instrument` | string | The venue symbol |
| `sequence` | uint64, optional | The venue's sequence, absent when it numbers nothing |
| `update` | string | `snapshot` or `delta` |
| `level` | uint32, optional | Position within the record |
| `side` | string, optional | `bid` or `ask` |
| `action` | string, optional | `set` or `remove` |
| `price` | decimal(18, price scale), optional | Level price |
| `quantity` | decimal(18, quantity scale), optional | Base quantity, absent on `remove` |

`facts` holds one row per market record that is not a book: trades, liquidations, funding rates,
open interest, mark and index prices. One table rather than one per kind: a reader filters by
`kind`. A column a kind does not use is absent in its rows, never zero.

| Column | Type | Content |
|---|---|---|
| `journal_seq`, `record_in_frame`, `received_at`, `exchange_time`, `instrument` | as in `book` | |
| `kind` | string | `trade`, `liquidation`, `funding`, `open_interest` (base quantity), `open_interest_value` (quote value), `mark`, `index` |
| `side` | string, optional | A trade's aggressor (`buyer`, `seller`, absent when the venue does not say) or a liquidation's forced order (`buy`, `sell`) |
| `trade_id` | string, optional | The venue's identity for a trade |
| `price` | decimal(18, price scale), optional | Trade or liquidation price, mark price, index price |
| `quantity` | decimal(18, quantity scale), optional | Trade or liquidation size, open interest in base |
| `value`, `value_scale` | int64 and its scale, optional | A figure with its own scale: the funding rate exactly as the venue wrote it, or open interest in quote |
| `next_funding` | timestamp (ns, UTC), optional | The next funding time, when the venue states it |

`gaps` marks where the record may be incomplete.

| Column | Type | Content |
|---|---|---|
| `journal_seq` | uint64 | Journal position of the entry that caused the mark |
| `at` | timestamp (ns, UTC) | When it happened |
| `kind` | string | `session-started`, `session-ended`, `continuity-lost`, `frame-refused` |
| `detail` | string | The refusal, for `frame-refused` |

Each file also carries key-value metadata: format version, table, venue, price scale and quantity
scale. A reader refuses a file whose schema or metadata differs from what it expects.

A session that has `session-started` and no `session-ended` ended abnormally. After
`continuity-lost`, each instrument is incomplete until its next snapshot.

## Integrity

- A file is written to a temporary name, synced, renamed and synced again. Only then is its
  manifest line appended. The final name only ever holds a whole file.
- On open, the writer removes temporary files, moves files no manifest line lists to
  `quarantine/`, and reports whether the previous session closed. A session that did not close
  lost its buffered rows from the archive, not from the journal. Rebuild with
  `bijective-data-engine --replay-store <store>` into a fresh archive.
- A stop request (SIGTERM or an interrupt) ends a live session cleanly: the session end is
  journaled and the archive sealed before the program exits.
- One writer per venue, enforced by `.writer.lock`. A stale lock is not broken automatically.
- `bijective-archive verify` checks that every listed file exists, hashes to its digest, reads under
  its schema and holds its row count, and that no file is unlisted. An empty archive does not
  verify as sound.
- `bijective-archive compact --table <book|facts|gaps> --date <YYYY-MM-DD>` merges a partition's files
  into one, reads it back and compares every row before the manifest supersedes the inputs and
  they are deleted. It takes the venue's lock, so it is refused while the data engine runs.
- Readers that want the newest facts (the console) order files by the window and journal entry in
  their names, not by manifest order: a compaction lists an old partition after newer segments.
  They take no lock and read sealed files only.

## Retention

The journal grows by the raw frame size, about 2 GB per day for two instruments at the recorded
rate. `bijective-journal prune` removes the journal segments whose frames a verified archive
already holds, so the journal holds the unarchived tail and the archive holds the history.

A prune removes nothing unless all of these hold:

| Condition | Refusal |
|---|---|
| The archive verifies (`bijective-archive verify` sound) | `ArchiveUnsound` |
| Every archive session before the current one closed, an unclosed one lost buffered rows that only the journal holds | `SessionUnclosed` |
| The journal entry at the archive's newest position carries the same instant as the archive row | `NotThisJournal` |

What is removed: every sealed journal segment whose last entry is before the archive's newest
entry, oldest first, never the newest segment. The archive's newest entry itself is kept. A prune
is safe while the data engine runs. The rotation bound `--journal-segment-bytes` (default 64 MiB)
is the granularity of what one prune can remove.

After a prune the archive is the record of the removed range: the journal can no longer rebuild
it, and `--replay-store` on a pruned journal replays from its horizon. A rebuild of the whole
archive is possible only from an unpruned journal or a backup taken before the prune.

## Bounds and measurements

| Quantity | Bound or measurement |
|---|---|
| Rows buffered before a seal | At most 100,000 (one segment) |
| Peak memory, recorded Bybit run (2,774 frames, 2 instruments) | 23 MiB measured on a debug build, the `Market archive` gate fails above 64 MiB |
| Journal cost | One sync of the journal per batch, then at most one each for the facts and the market stream |
| Journal throughput, one instrument, 2026-11-27 | 496,461 recorded Bybit linear frames replayed on a WSL 2 host: 4,370 frames/s per batch, 163 frames/s with a sync per entry, 5,150 frames/s with syncs that cost nothing (tmpfs). The session was recorded at 65 frames/s |
| Journal throughput, same run (undated, before decision 0019) | 2,774 frames in 1.05 s on a release build (about 2,600 frames/s), the venue sent them at 62 frames/s. A sync costs far more on some hosts: see the 2026-11-27 row |
| Journal size, same run | 1.1 MB: the frames as received, uncompressed |
| Archive size, same run | 1.1 MB of frames became 172 KB of book rows (22,112 rows) |

A record kind with no columns yet (the long/short ratio, which no adapter produces) is counted as
not archived and left to the journal, which holds its frame. A later archive with its columns is
rebuilt from the journal, so nothing is lost. The count is in the data engine's readiness detail.
