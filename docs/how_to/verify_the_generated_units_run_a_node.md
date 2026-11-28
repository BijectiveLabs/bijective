# Verify the generated units run a node

Platform-Version: 0.1.1
Drill: scripts/verify/units.sh
Applies-To: before a node is installed from a new release, and after any change to the manifest or
            to the unit generator

**Why:** the units are generated and never hand-maintained, and until now nothing ran one. The
boundary drill starts the programs directly, so the whole unit layer - the hardening, the writable
paths, the delegated subtree, the ordering - was checked by reading it. That is how a unit set that
granted no writable path at all sat in the repository: every program writes a store and a readiness
file, `ProtectSystem=strict` mounts the hierarchy read-only, and no test ever asked a unit to do it.

## Procedure

```
./scripts/verify/units.sh
```

No cgroup subtree is needed. It writes one unit into the user unit directory, starts it, and removes
it again; it refuses rather than overwrite a unit of that name that already exists.

## Reading the result

| Output | Meaning |
|---|---|
| `every program that declared a writable path was granted it` | The manifest and the units agree |
| `the supervisor's unit carries its delegated subtree and a declared tolerance` | It can bound a run, and how far behind a strategy may fall is stated |
| `installed stopped` | No session begins because a node booted |
| `the init system accepts the generated unit as written` | `systemd-analyze verify` found nothing wrong with our file |
| `the unit started its program, and the program wrote what it was granted` | A generated unit carries a real program |
| `NOT VERIFIED  this user manager applies the unit's mount namespace ...` | The unit exited 226 (NAMESPACE): the user manager applies `PrivateTmp` and `ProtectHome`, so the drill's workspace under `/tmp` and a build under `/home` are hidden from it. [Rehearse an install on this host](rehearse_an_install_on_this_host.md) starts the shipped units as system units instead |
| `a program whose store it cannot write refuses at start, naming the path` | If a grant is ever wrong, an operator is told at start rather than at the first decision |
| `the hardening is in force (system manager) ...` | A write outside the granted paths was refused: the grant is doing work |
| `NOT VERIFIED  no manager here applies ProtectSystem ...` | Neither the user manager nor a privileged one could show a refusal; the grants are declared and unexercised |

**The hardening line is the one to read carefully.** A user manager without the privileges to set up
a mount namespace takes `ProtectSystem=strict` and does nothing with it, so a write outside the
granted paths succeeds and the granted paths prove nothing. The drill probes for that by trying it -
under the user manager, and then under the system manager where privilege is already available
without a prompt, in a transient unit that installs nothing and leaves nothing behind.

Measured 2026-09-04: under a system unit, a write outside the granted paths fails with *Read-only
file system*, and with no grant at all so does a write inside it. **The grant is necessary and
sufficient**, and a unit set without one cannot write a store.
