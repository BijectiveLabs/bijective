# Decide how long a strategy may pause

Platform-Version: 0.1.0
Drill: scripts/measure/backpressure.sh
Applies-To: before a paper or live session on a venue this node has not carried before, and after
            any change to the queue behind a live strategy

**Why:** a live session hands its strategy every event as it is recorded. If the strategy stops
taking them the queue fills, and the session ends on the first event it cannot deliver - naming the
position, loudly. That is the right failure. What an operator needs to know first is **how long a
strategy may pause before it happens**, because the answer depends on the venue's arrival rate and
not on anything the platform chooses.

## Precondition

The same delegated cgroup v2 subtree [Verify an agent run can be bounded](verify_an_agent_run_can_be_bounded.md) describes.

## Procedure

```
systemd-run --user --scope \
  --setenv=BIJECTIVE_CGROUP_ROOT=/sys/fs/cgroup/<delegated-subtree> \
  -- ./scripts/measure/backpressure.sh 10 100 1000
```

The arguments are **arrivals per second** for the venues and instruments actually in play. Read the
first half off a market this node has recorded rather than inventing it:

```
measure_arrivals <store> <market-stream> [queue-bound]
```

Measured 2026-09-04 on one perpetual instrument at one venue: **47 events a second on average, 50 in
the busiest second**. A session reads one stream per venue, so multiply by the instruments the deployment
subscribes to - twenty of them is about a thousand a second, which is one second of stall
tolerance against a queue of 1,024.

## Reading the result

| Output | Meaning |
|---|---|
| `HOLDS n` | Events the platform absorbs for a strategy that has stopped reading |
| `DRAINS n/s` | Events a reading strategy takes, answer included, on this machine |
| `slack` | How long that strategy may be stalled at the stated arrival rate |
| `a countdown, not slack` | Arrivals outrun the strategy: the queue is emptying into a wall |

Two cores measured 1,024 held and about 58,000 drained a second. At a hundred arrivals a second a
strategy may pause ten seconds; **at a thousand it may pause one**, which is inside a garbage
collection or a slow log write.

**A session that ends this way is not a fault to suppress.** It means the strategy stopped
participating in the market it was being sent, and continuing would leave its proposals resting on a
stream with holes in it. If the slack is too tight for the venue in question, that is a decision
about the bound, taken with these figures in hand.
