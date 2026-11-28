# Try the platform without installing it

Platform-Version: 0.1.1
Drill: scripts/verify/guided_path.sh
Applies-To: evaluating the platform, before any decision to install one

**Why:** deciding whether this platform is worth installing should not require installing it.
[Verify a release before installing it](verify_a_release.md) and [Install a node and bring it up](install_and_start_a_node.md) are for a node that will run; this is for a laptop, a workstation or a shared
host used to write a strategy and see a verdict.

Nothing here is a reduced mode. The run is bounded by the same sandbox, the proposals go on the same
streams, and the promotion stages answers the same way.

## What is needed

- The release artefact, unpacked anywhere. Nothing is installed, nothing is copied into `/opt`,
  `/etc` or `/var`, and no root is required.
- A host whose init system can start a **user scope**. Every run is bounded by memory and time
  limits, so where `BIJECTIVE_CGROUP_ROOT` names a delegated subtree that one is used, and
  where nothing is named the command asks for a scope of its own and says which subtree it made.
  What it will never do is adopt a subtree delegated to another process.

## Procedure

```
export PATH=<artefact>/bin:$PATH

bijective-backtest --new alpha.py
bijective-backtest alpha.py
bijective-promote store alpha.py 19600
```

On a host where an operator has already delegated a subtree, name it and it is used instead:

```
export BIJECTIVE_CGROUP_ROOT=/sys/fs/cgroup/<delegated-subtree>
```

The first command writes a strategy that already runs, seeds a **sample market** so no venue is
needed, and writes `bijective.json` - so the second command needs no arguments beyond the strategy.

## What the sample market is, and is not

It is a generated book, on a stream named `market-sample`, and the run's ledger entry records that
name. **It is not a venue recording**, no claim measured on it says anything about a real market,
and it is never added to a store that already holds one.

Recording a real market is [Install a node and bring it up](install_and_start_a_node.md)'s business, and it needs a venue, a profile and a credential.

## Reading the result

The second command prints every choice it made - the market, the window, the exchange, the library
it declared, and the supervisor command it ran. The third says where the strategy stands on the path
to capital, and what the next thing to do about it is.
