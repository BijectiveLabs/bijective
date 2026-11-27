# Prune the frame journal

Platform-Version: 0.1.0
Drill: scripts/verify/market_archive.sh
Applies-To: a data engine node whose journal disk use must be bounded

## Why

The frame journal grows by the raw frame size, about 2 GB per day for two instruments. Once a
verified archive holds a frame's records, the journal copy is no longer the only record of it.
Pruning removes those journal segments and keeps the unarchived tail.

## Steps

1. Take a backup first if the whole archive must stay rebuildable from raw frames: after a prune,
   the journal cannot rebuild the removed range.
2. Run, with the price and quantity scales of the venue profile the data engine reads (the
   archive holds one pair per venue, decision 0007):

   ```bash
   bijective-journal prune --store <store> --venue <venue> --archive <archive> \
       --price-scale <n> --quantity-scale <n>
   ```

3. Safe while the data engine runs. Schedule it as often as disk use requires; a prune that finds
   nothing to remove exits 0 and says `pruned 0 segment(s)`.

## Reading the outcome

| Exit | Output | Meaning |
|---|---|---|
| 0 | `pruned <n> segment(s) ... the journal begins at entry <h>` | Segments removed; reads below `<h>` are refused |
| 1 | `nothing removed: the archive did not verify` | Run `bijective-archive verify` and resolve its findings |
| 1 | `nothing removed: an earlier archive session never closed` | Rebuild the archive from the journal with `bijective-data-engine --replay-store`, then prune |
| 1 | `nothing removed: ... it was written from another` | The `--archive` given was not written from this `--store` |
| 2 | Usage message | A flag is missing or malformed |

The refusals are the fail-closed path: a journal is never pruned on an archive that cannot be
shown to hold what is removed.
