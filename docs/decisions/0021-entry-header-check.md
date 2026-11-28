# 0021: A check over each entry header, event store format 2

| Field | Value |
|---|---|
| Status | Proposed, 2026-11-28. Not built |
| Extends | The event store's on-disk format ([event sourcing](../concepts/event_sourcing.md)) |

## Context

An entry header is a marker, a payload length, a position and a link. The link covers the
position, the length and the payload, so damage to any of them is found, except where the length
itself sends the reader past the end of the segment. A length that runs past the end looks like an
interrupted write, which recovery seals off and leaves on disk.

Two checks close most of that gap without a format change (2026-11-28): an entry whose own link
verifies at the end of the segment, or where a header for the next position begins, is whole and
its length damaged (`PayloadLengthDamaged`), and a sealed segment is held to its manifest
(`ManifestMismatch`). One case remains: the active segment, unsealed, with its length damaged
**and** its payload damaged as well. Neither check can tell that from a torn write. Recovery then
seals before that entry and reuses its position, so an acknowledged entry, and every one after it,
leaves the record.

## Proposal

1. **Format 2 adds eight bytes to each entry header:** the first eight bytes of SHA-256 over the
   marker, the length and the position. The header becomes 54 bytes. SHA-256 is already in the
   crate, so no dependency is added.
2. **A torn tail is redefined by the check.** A header that is whole and whose check verifies,
   followed by a payload that is short, is a torn tail. A header whose check does not verify is
   `Corrupt` (`HeaderCheck`), wherever it lies. A header that is itself cut short is a torn tail,
   as now.
3. **Format 1 stays readable.** The segment header already carries the format version. A format 1
   segment is judged as it is today. A writer opens every new segment in format 2, so a stream
   moves to format 2 at its next rotation or recovery, and no segment is rewritten.
4. **Cost.** One SHA-256 over 14 bytes per append, measured before acceptance against the journal
   rate of decision 0019 (4370 frames per second on the 2026-11-27 corpus).

## What it rules out and what it does not

It rules out every damaged length, alone or with its payload, in any segment. It does not protect
against a writer that is wrong: a check computed over wrong fields verifies. The link and the
manifest remain the guarantee for content.

## Shorter path

Leaving format 1 as it is. The residual case needs two independent faults in one entry of the one
unsealed segment, and the venue remains the source of truth for orders and fills, so reconciliation
finds a lost fill at the next pass. The cost of the shorter path is that the record, which is the
audit trail, can lose an acknowledged entry without saying so.
