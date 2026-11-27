# Execution Quality

Execution quality attribution answers what an execution actually cost against the price that was
achievable at the moment the strategy decided, and which causes account for the difference: spread,
queue, adverse selection, fees and funding. An edge can be real and still be consumed entirely by
execution, and a fills report alone does not show it.

## Decision capture

`bijective-decision-capture` records what the market looked like at the instant a strategy decided.

- The control engine keeps a bounded top-of-book history per instrument, limited by instrument
  count, by age and by length (4096 snapshots). Under pressure it degrades and counts refusals
  rather than growing. The history reaches back the freshness bound plus two heartbeats, because a
  proposal is judged up to a pass after the strategy wrote it, when newer books have arrived.
- The benchmark is the newest book this host had received at the decision instant, keyed on the
  receipt time, the clock the strategy's `decided_at_ns` is read on. The venue's own stamp is not
  used: it would admit a book that had not yet arrived. A book that left the history is recorded
  as `SnapshotEvicted`, not as a missing one.
- The control engine stamps a benchmark for every proposal it acts on, before the order goes,
  against the strategy's own `decided_at_ns`, never the submission time. Using the submission time
  would fold the platform's own latency into the benchmark.
- A decision with no book behind it is recorded as having none. An absent book is never written as
  zeroes.
- A snapshot newer than the decision is refused as a clock inconsistency.
- A benchmark is written for refused proposals too: the record is about the decision.
- Capture is constant work per call, and the crate has no I/O and no dependencies.

## Attribution

`bijective-attribution` (Python) is the read-side pipeline: fill correlation, the attribution
calculator, the store and roll-up, and the capability gate.

- It subscribes to a copy of the event flow and has no write path to the order path. A defect here
  can produce a wrong figure but cannot alter an order or a position.
- Where an input is unavailable the result is "insufficient evidence", with the reason. Never a
  default, a zero or a nearest value.
- Fills are correlated to decisions by client order identifier.
- The store is optional; a PostgreSQL store is available through the `postgres` extra.

## Execution cost in the console

The console's Execution cost table (`/api/panels/execution-cost`) reads the same definitions
from the record alone: one row per order, against the mid of the book captured at its decision.

| Column | Meaning |
| ------ | ------- |
| `bid at decision`, `ask at decision` | The captured book, at the instrument's price scale |
| `average price` | Fills' volume-weighted price, rounded half away from zero four digits past the price scale |
| `arrival cost` | What the fills paid beyond the mid, positive a cost, exact, in the quote currency |
| `arrival cost bps` | The arrival cost over the mid's value of the filled quantity, to a hundredth |
| `crossing cost` | Mid to touch on the filled quantity: what immediacy costs |
| `residual cost` | Touch to fill: the arrival cost less the crossing |
| `decision to first fill` | From the decision instant to the first fill's receipt |
| `no benchmark because` | Why no book was captured; every cost is then absent, never zero |

Fees are not in the arrival cost: the Fills table holds them in the asset they were charged in.
In a paper run a resting or taking order meets the market at the next pass, so its decision to
first fill is about one heartbeat (5 s) and its arrival cost is mostly the market's move over
that time, not the venue's execution.
