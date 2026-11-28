# Verify an agent run can be bounded

Platform-Version: 0.1.1
Drill: scripts/verify/sandbox.sh
Applies-To: before the Strategy Engine supervisor is enabled on a node, and after any change to
            the node's cgroup configuration

**Why:** the supervisor refuses to start if it cannot bound a run, and that refusal reads as a
program that will not come up. This drill tells the two apart before an operator is debugging a
service at the wrong layer.

## Precondition

The node must delegate a cgroup v2 subtree to the account the supervisor runs as. Under systemd
that is `Delegate=yes` on the unit, which `bijective-deploy` emits for this program and no other.

## Procedure

The drill must run **inside** the delegated tree, not merely point at it: moving a process between
cgroups needs write access to the source as well as the destination, and a login session's scope
belongs to root.

```
systemd-run --user --scope -p Delegate=yes --expand-environment=no -- bash -c \
  'BIJECTIVE_CGROUP_ROOT=/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup) bash scripts/verify/sandbox.sh'
```

The scope is the delegated subtree, and the drill reads its path from `/proc/self/cgroup`. A cgroup
v2 node cannot both hold processes and enable controllers for its children, so the drill moves the
scope's processes into a `leader` leaf before it enables `memory` and `pids`, as the supervisor does
at start. Measured 2026-11-27 on WSL 2 with systemd 255: three `PASS` lines.

## Reading the result

| Output | Meaning |
|---|---|
| Three `PASS` lines | The bound is in force, the cgroup is removed with the run, limits are per run |
| `NOT VERIFIED` | No subtree was named. The drill will not guess one; limits are unproven until it runs |
| `FAIL ... exit 111` | The run could not enter its cgroup. The drill is not inside the delegated tree |
| Supervisor refuses: `a process could not be moved into ...` | The same cause, found at start: the supervisor moves a probe process in as it would a run. Start it from inside the delegated tree, not from a `su` or `ssh` session, which systemd places in a scope of its own |
| `FAIL ... the bound was not in force` | The subtree is not a cgroup, or the memory controller is not delegated |

**A `NOT VERIFIED` is not a pass.** The isolation half of the sandbox is a change gate and runs
everywhere; this is the half that needs a real node, and until it runs on one the memory bound is
implemented and unproven.
