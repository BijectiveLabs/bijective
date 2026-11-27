# Install a node and bring it up

Platform-Version: 0.1.0
Drill: scripts/verify/boundary.sh
Applies-To: a fresh host, after [Verify a release before installing it](verify_a_release.md) has passed

## Why

Units are generated from one manifest and never hand-maintained, and the operational programs are
installed **stopped**. The core starts, ingests and runs without them, and that is
what the drill for this guide checks.

## Steps

1. `bijective-deploy generate <manifest.json> <unit-dir>`. It prints which units to enable and
   which to install stopped. It does not enable anything itself: enabling a unit changes the host,
   and a generator that also changes the host cannot be run only to inspect its output.
2. Install the generated units into the init system's unit directory.
3. **[Verify the generated units run a node](verify_the_generated_units_run_a_node.md), on this host, before anything is enabled.** It is where the unit layer is checked
   against a real init system: whether the units grant the paths their programs write, whether the
   init system accepts them, and whether one actually starts its program. A developer machine cannot
   settle the last of those and says so; this host can.
4. Enable and start only the units it listed as `enable`.
5. `bijective-deploy status <manifest.json> <run-dir>` until it reports `Ready`.

## The Strategy Engine is installed and does not start

It is an operational unit, so it is installed stopped and stays that way. A live session must not
begin because a node booted, and the unit cannot know what to run: *which* strategy and *which*
stage come from the operator's environment, and an unset stage is refused rather than assumed.

Before starting an instance the host must provide, as environment for that unit:

| | |
|---|---|
| `BIJECTIVE_CGROUP_ROOT` | the delegated cgroup v2 subtree ([Verify an agent run can be bounded](verify_an_agent_run_can_be_bounded.md)) |
| `BIJECTIVE_STRATEGY` | the program to run |
| `BIJECTIVE_MARKET` | the market stream it reads |
| `BIJECTIVE_STAGE` | `paper` or `live` - never left unset |

A strategy written in Python needs the protocol library where the run can import it. The release
artefact carries it as `lib/python/bijective_strategy`, next to `bin/`: `bijective-backtest` looks
there and declares it to the run, and refuses to start a Python strategy when it is absent rather
than letting the import fail inside the sandbox. A node built from source installs it there from
`python/bijective-strategy/src/bijective_strategy`.

The manifest also declares `--events-behind`, which is how far behind the strategy may fall before
the session ends. It is an operator's tolerance rather than a platform constant: [Decide how long a strategy may pause](decide_how_long_a_strategy_may_pause.md) turns it
into seconds for the venue in question, and the session's summary line reports the number in force
and whether anybody declared it.

## What Ready does and does not mean

**Ready means every core program has finished starting.** It does not mean the platform is trading:
the trading state rests at *Halted*, and the Control Engine's detail line says so.

**Do not read the init system for readiness.** A program running is not a program ready - the
Control Engine is alive throughout its limit-state replay and must not be treated as ready during
it.
