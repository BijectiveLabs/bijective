# Rehearse an install on this host

Platform-Version: 0.1.1
Drill: scripts/verify/node.sh
Applies-To: a host that will run the platform, before the first real install, and after any change
            to the manifest, the unit generator or a program's start-up

**Why:** [Verify the generated units run a node](verify_the_generated_units_run_a_node.md) starts one generated unit under whichever manager it can reach. This installs the
**shipped** unit set as system units at the paths the manifest names - `/opt/bijective`,
`/etc/systemd/system`, `/var/lib/bijective`, `/run/bijective` - brings a program up under one, and
removes everything again. It is the only procedure that exercises what an operator actually does.

## Precondition

Root, without a prompt. **Nothing may already be installed**: the drill refuses if any of those
paths exists, because a drill that removed them on the way out would remove another node's files.

## Procedure

```
./scripts/verify/node.sh
```

## Reading the result

| Output | Meaning |
|---|---|
| `n file(s) verified before anything was unpacked` | The artefact was recorded and verified before install |
| `the init system registered the ordering and the supervisor's delegated subtree` | `Requires=`/`After=` and `Delegate=yes` landed as written |
| `the supervisor is installed and not enabled` | The init system's own answer, not a line in a file |
| `a program under a shipped unit started and wrote its readiness` | A shipped unit carries a real program at the real paths |
| `the same unit without its grant wrote nothing` | `ReadWritePaths` is what makes the unit work, shown by removing it |
| `the Control Engine came up under its shipped unit, and came up halted` | The credential reached the broker, the keystore opened, and the engine refuses to trade until told |
| `CLEAN the host is as it was` | Nothing was left behind - units, files or loaded state |

## What it does not do

It does not start the Data Engine, which needs a venue and an endpoint. The Control Engine **is**
started, with a **drill credential that is valid at no venue** - a key that cannot be proven to be a
demo key has to be treated as a live one, and a live one has no place in a start-up rehearsal.

**If `CLEAN` does not appear, stop and look.** The first run of this drill left two units loaded as
`not-found`, because `systemctl disable` reads a unit to find its `[Install]` section and the files
had already been removed. The order is: stop, disable, then remove.
