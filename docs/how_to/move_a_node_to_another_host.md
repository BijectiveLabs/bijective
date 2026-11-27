# Move a node to another host

Platform-Version: 0.1.0
Drill: scripts/verify/migration.sh
Applies-To: moving a node to new hardware, a new provider, or a rebuilt host

**Why:** the restore drill proves a backup comes back - that is one host recovering itself. A move is
a different claim. The record arrives somewhere else, the strategies sit at different paths, the
credentials are **not** carried, and what an operator reads afterwards has to be what they read before.

## What moves, what is made again, and what must never travel

| | |
|---|---|
| **Moves** | the event store; the deployment manifest; the node configuration; the release artefact |
| **Made again on the new host** | the keystore and its credentials ([Load a venue credential](load_a_venue_credential.md)); the delegated cgroup subtree ([Verify an agent run can be bounded](verify_an_agent_run_can_be_bounded.md)); the unit files, generated from the manifest ([Install a node and bring it up](install_and_start_a_node.md)) |
| **Never travels** | a keystore, an init credential, or anything else secret. A store holds the record and no secret, and the drill looks for one rather than assuming |

**Credentials are re-loaded, not copied.** A key that travelled with a backup would reach every host
that ever held one, including the ones a move was meant to leave behind. Loading them again is also
the moment to rotate them, and the venue is the only place that can say a key still works
([Verify a venue still behaves as declared](verify_a_venue_still_behaves_as_declared.md)).

## Procedure

1. **On the old host**, stop the units, then take the store. `bijective-deploy status` first, so what
   was running is written down before it stops.
2. **On the new host**, in this order:
   - [Verify a release before installing it](verify_a_release.md) - verify the release artefact **before** unpacking it;
   - [Install a node and bring it up](install_and_start_a_node.md) - install, which includes [Verify the generated units run a node](verify_the_generated_units_run_a_node.md) before anything is enabled;
   - [Verify an agent run can be bounded](verify_an_agent_run_can_be_bounded.md) - delegate a cgroup subtree;
   - [Load a venue credential](load_a_venue_credential.md) - load the credentials, fresh;
   - put the store in place, then [Restore the store from a backup](restore_from_a_backup.md)'s verification that it reads.
3. **Prove it is the same node.** For each strategy that matters:

```
bijective-report <store>                       # the same strategies, most recent first
bijective-report <store> <name> <confidence>   # the same page
bijective-promote <store> <name> <confidence>  # the same answer
```

**Compare them with what the old host printed.** The store's own path appears in a heading and is
expected to differ; nothing else should.

## Why a strategy is still found after the move

Its identity is the name declared in `bijective.json`, not the path of its file. A
strategy that lands in a different directory, or under a different file name, is the same strategy
with the same history. Under a path identity a migration would have looked like a fresh start, with
every stage sequence empty and no warning that anything had been lost.

What the move does **not** carry over is a clearance across a change of bytes: if the program itself
changed on the way, the promotion stages says so and names both versions.

## If a page differs

Do not continue. A figure that depends on where a store sits is a figure about the filesystem. Take
the two pages, find the line that differs, and treat it as a defect in the record rather than as a
difference between hosts.
