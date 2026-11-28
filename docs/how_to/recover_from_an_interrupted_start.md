# Recover from a start that was interrupted

Platform-Version: 0.1.1
Drill: scripts/verify/interrupted_start.sh
Applies-To: after a crash, a power loss, or a kill during startup

## Why

The dangerous moment is not the crash. It is the **restart**: a platform that brought the order path
up before its limit state had replayed would be trading on limits an operator has declared in force
and which hold no consumed budget.

**Nothing has to happen for the platform to be safe.** *Halted* is where the trading state rests, so
an interrupted start holds safe by default rather than by a step succeeding - there is no halt step
to check, and that is deliberate: a step that must run to make a system safe is a step that can be
skipped.

## Steps

1. `bijective-deploy status <manifest.json> <run-dir>`. Expect `Failed`, naming the program that
   stopped reporting.
2. Start the failed program again. It replays its state and verifies it before reporting ready.
3. Wait for `Ready`. **Do not shorten this by treating the init system's report as readiness.**
4. Confirm the Control Engine's detail line still says trading is halted. It should - nothing in a
   restart resumes trading.

## If replay refuses

The Control Engine reports `replay did not complete, so no limit is known to be in force` and does
not come up. **This is correct behaviour, not a fault to work around.** Run [Restore the store from a backup](restore_from_a_backup.md) against the
most recent verified backup rather than starting the engine with the replay bypassed; there is no
bypass, and adding one would remove the property this guide exists to rely on.
