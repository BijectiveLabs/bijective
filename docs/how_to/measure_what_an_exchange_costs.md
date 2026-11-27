# Measure what an exchange costs on this node

Platform-Version: 0.1.0
Drill: scripts/measure/exchange_cost.sh
Applies-To: before a long backtest on a node the platform has not been measured on, and after any
            change to the supervisor's exchange handling

**Why:** a bounded run declares how it is fed. Stepwise spends a turn per event so
the supervisor can place a decision; whole spends none because it cannot place one. Both are
bounded by the same wall, so on a large enough window the question stops being which one is
preferable and becomes which one finishes. That answer is a property of the node, not of the
platform, and it is measured before the backtest than after it.

## Precondition

The same delegated cgroup v2 subtree [Verify an agent run can be bounded](verify_an_agent_run_can_be_bounded.md) describes. A run that is not bounded is not the run
an operator gets, and this will not guess a subtree.

## Procedure

```
systemd-run --user --scope \
  --setenv=BIJECTIVE_CGROUP_ROOT=/sys/fs/cgroup/<delegated-subtree> \
  -- ./scripts/measure/exchange_cost.sh 1000 16000
```

Window sizes are arguments. Give it the shape of the backtest that is actually planned: the fixed
cost of starting a run dominates a small window, so a rate taken at a thousand events understates
what a hundred thousand will do.

## Reading the result

| Output | Meaning |
|---|---|
| `MEASURED` | Both exchanges carried every window inside the wall bound. The rates are this node's |
| `NOT MEASURED` | No subtree was named. Nothing was run |
| `FAIL ... the run did not carry the window` | That exchange could not finish that window here. The rows above it are what an operator would see |
| `at that price the wall bound is reached at about N events` | The window size at which the run is cut short rather than completed |

**A window that does not fit is not a slow backtest - it is no backtest.** A run cut short admits
nothing, so what it reached is not a result and never reaches the proposal stream.

The figures printed include the machine, because they are the machine's. Two cores measured 17,000
events a second whole and 8,600 stepwise; a node with faster cores will differ, and a node under
load will differ more.
