# Restore the store from a backup

Platform-Version: 0.1.1
Drill: scripts/verify/restore_drill.sh
Applies-To: recovery, and the periodic rehearsal of it

## Why

A backup that has not been read is not a backup. The restore is verified with **the same verifier
the live store uses**, because a second implementation of *is this store intact* is a second answer
waiting to disagree.

## Steps

1. `bijective-deploy restore-drill <backup-dir> <restore-into-dir>`.
2. The target must be **empty or absent**. A drill never writes over existing state.
3. The command proves its own refusals before reporting a pass - it corrupts a copy of what it has
   just restored, once in a payload byte and once in a length field, and requires itself to refuse
   both. It also proves a non-empty target is refused. All three `PROVEN` lines must appear.

## Reading the outcome

| Line | Meaning |
|---|---|
| `PASS ... verified clean` | The copy is intact |
| `NOTE ... end mid-entry` | Clean **and not a complete copy**. The backup was taken while a write was in progress; check when it was taken |
| `did not verify` | Corruption inside a complete entry, or a sealed segment shorter than its manifest. **Nothing may resume onto this.** Keep the copy - it is evidence |

**Clean and complete are different facts.** A torn tail on a live node is the crash signature the
store preserves on purpose; on a restored copy it is the difference between a backup that captured
everything and one taken mid-write.
