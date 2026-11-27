# Order Lifecycle

`bijective-lifecycle` defines what an order, a fill and a position are, as pure state machines.
Nothing in it reads a clock, opens a file or names a venue, so the order path, reconciliation and
replay fold the same events to the same state.

## States

```
Created -> Submitted -> Accepted -> PartiallyFilled -> Filled
               |            |              |
               |            +-> CancelPending -> Canceled
               +-> Rejected                 |
               +-> Canceled / Expired       +-> back to Accepted or PartiallyFilled (cancel rejected)
               +-> NotSent
               +-> Unknown -> Reconciling -> Accepted | PartiallyFilled | Filled | Rejected | Canceled | Expired
```

| State | Meaning | May still execute |
|---|---|---|
| `Created` | Cleared and recorded, not yet sent | No |
| `Submitted` | Sent; no answer yet | Yes |
| `Accepted` | Acknowledged by the venue | Yes |
| `PartiallyFilled` | Some quantity executed, the rest working | Yes |
| `CancelPending` | A cancel was sent and not yet confirmed by the venue | Yes |
| `Filled` | All quantity executed; at least one fill recorded | No (terminal) |
| `Rejected` | The venue refused it; nothing executed | No (terminal) |
| `Canceled`, `Expired` | Stopped working; late fills are still counted | No (terminal) |
| `NotSent` | The call was held back before it left (the venue's request budget or circuit breaker); no venue held it | No (terminal) |
| `Unknown` | The answer could not settle whether it reached the venue | Yes |
| `Reconciling` | Reconciliation is establishing its state from the venue | Yes |

`Unknown` is a state, not an error. A timeout, a dropped connection or an unreadable answer moves
an order there, and only the venue's evidence moves it out: an acknowledgement, a fill, a cancel,
an expiry or a rejection, directly or after reconciliation starts. An unknown order is never
treated as filled or as failed, and it keeps its budget reservation.

The full table, every state against every event, is checked cell by cell in
`crates/lifecycle/tests/unit/order_tests.rs`.

## Events and audit records

Each applied event returns the new order and one `Transition`:
`(from, to, cause, detail, venue_order_id, at_ns)`. Cause names follow the order event chain:
`order_submitted`, `order_acked`, `order_rejected`, `order_filled`, `order_canceled`,
`order_expired`, `cancel_requested`, `cancel_rejected`, `outcome_unknown`,
`reconciliation_started`, `order_not_sent`. A refusal returns an error and leaves the order as it
was.

## Fills

A fill carries the venue's execution identifier, the side, the price, the base quantity, the fee in
its own asset, the liquidity side, the exchange time and the arrival time.

- **Delivered twice is a duplicate.** A fill whose identifier was applied with the same content
  changes nothing. The same identifier with different content is refused as a conflict.
- **No overfill.** A fill that would take the filled quantity past the ordered quantity is refused.
- **No contradiction.** A fill on the other side, with a different venue order identifier, with a
  non-positive figure or without an identity is refused.
- **Fills that cross a cancel count.** A fill on `CancelPending`, `Canceled` or `Expired` updates the
  filled quantity and becomes `Filled` if it completes the order.
- `Filled` always has at least one fill, and the filled quantity is the sum of the fills.

## Positions

A position is netted per instrument and changes only by a fill or by an audited adjustment
(`PositionCause::Adjustment`, with an identity and a reason). Each identity is applied once.

- Open quantity is held as lots in the order they opened, each at its own price. A reduction
  consumes the oldest lots first.
- Realized profit is the exact sum of (exit - entry) times the quantity closed, for longs, and
  (entry - exit) times the quantity closed, for shorts. No average price is rounded into the
  record; `open_cost_scaled` gives the exact cost of what is open, and a reader that wants an
  average divides it itself.
- A fill larger than the position closes it and opens the remainder on the other side at the
  fill's price.
- Figures are scaled integers. Realized profit is in quote units at the price scale plus the
  quantity scale, for linear instruments. A product or sum beyond 128 bits is refused.
- Fees are kept per asset and scale beside the position, never netted across assets.

`Ledger` holds orders and positions together: a fill moves the order and its position, or, if
either refuses, neither.

## Reconciliation against venue evidence

`reconcile` reads venue evidence (order status reports, fill reports, position reports and what
the evidence covers) against a ledger and returns a reconciled copy, the transitions it applied,
its findings and a verdict. It runs in three phases: orders (acknowledged or refused), fills, then
terminal statuses and positions, so a position is judged on orders and fills already brought up to
date and a late fill counts before a cancel closes its order.

| Evidence | Outcome |
|---|---|
| An unknown order the venue reports | Enters `Reconciling`, then the reported state through the state machine |
| An unknown order the venue does not mention | Stays unknown; finding `Unresolved`. Absence is not failure |
| Filled quantity above the fills that explain it, the venue having changed the order within 180 s | Finding `FillsPending`, and `PositionAwaitingFills` for a differing position on that instrument; verdict `Unverified`. A venue publishes an order status before the executions that explain it |
| Filled quantity above the fills that explain it, older or without a venue stamp | Finding `MissingFills`; no fill is invented |
| Filled quantity below what the ledger holds | Finding `StaleReport`: an older snapshot never undoes a fill |
| An order or fill with no order of this platform | Finding `External`: capital moved unseen |
| An explicit position that differs | Finding `PositionMismatch` |
| A ledger position the venue did not report | Finding `PositionUnconfirmed`: a missing report is not flat |
| No positions published, or incomplete evidence | Verdict `Unverified`, never `Aligned` |

The verdict is `Aligned` only on complete evidence with no finding but a stale report. Evidence is
never adopted past the state machine, and nothing is resolved by policy: an order stays unknown
until the venue's evidence settles it.

A venue's private order stream does not bypass this. `VenueExecution::pushed` only says the venue
reported a change, and the node reads that venue's evidence on its next pass instead of waiting for
the reconciliation interval. A stream that lapses leaves the interval as the reader: it costs latency,
never a fill.

## In the control engine

The control engine folds its own stream into a ledger at start: an intent is `Submitted`, the
venue's answer moves it (a duplicate answer is `Accepted`, because the venue already holds the
order), and a withdrawal is `CancelPending` until the venue says otherwise. A record the state
machine refuses becomes a finding. The readiness detail reports how many orders may execute, how
many are unknown and awaiting reconciliation, and how many findings there are.

A resume command is refused while any order is unknown or reconciling: an unknown order may have
executed, and resuming would trade on a position the platform does not know. The refusal is recorded with the
count, and the platform stays halted.

Fills are recorded on the control stream as `Filled` events by `OrderPath::record_fill`, which
checks the fill against the order's lifecycle first: a fill already recorded is a duplicate and is
not written again, a fill for an order this platform did not place is not recorded as its own, and
a fill the lifecycle refuses (overfill, contradiction) is not written. On restart the projection
rebuilds each order and its position from these events. Reconciliation produces them, read on
the reconciliation cadence or sooner when a venue's private stream reports a change.

## Reconciliation at start

The control engine keeps one ledger per venue, taken from the venue each intent names: instrument
symbols are not unique across venues, so one ledger would add two venues' positions under one key.
When a venue opens, the orders the record routed to it are handed to its adapter
(`VenueExecution::adopt`): a venue that identifies an order only by the hash of its signed content
derives that hash again from the recorded request and instant, so its evidence names the order as
this platform's and not as external.

In a live run, before the first decision pass, each open venue's evidence (`VenueExecution::evidence`,
reaching back to a minute before that venue's latest aligned pass, or before the first intent
routed there where no pass agreed later) is reconciled against that venue's ledger, and the outcome
is written to the control stream. An aligned pass found every order it was shown explained by its
fills, so nothing before it is still owed, and a node that runs for months reads what changed since
its last agreement rather than its whole history. The minute is for the two clocks: an
intent carries this node's instant and a venue stamps its orders and fills on its own, so an order
can carry an earlier stamp than the intent that placed it. An order or fill of another client in
that minute reads as external, as any other would. The events written:

| Event | Class | Content |
|---|---|---|
| `Filled` | `OrderFilled` | A fill the evidence proved |
| `Reconciled` | `OrderReconciled` | A state change the evidence settled: reconciliation started, accepted, rejected, canceled, expired |
| `ReconciliationCompleted` | `ReconciliationCompleted` | The verdict (`Aligned`, `Unverified`, `Diverged`) and one sentence per finding |

The events are those the pass applied, in order, so the projection rebuilt at the next start equals
the reconciled ledger. Fills are applied in the order they executed at the venue (venue stamp, or
receive time where none is published, then fill identity), so positions built from FIFO lots do
not depend on the order an adapter listed them in.

Every pass records how its evidence read went:

| Read | Meaning | Resume |
|---|---|---|
| `Read` | The evidence was read and reconciled | Decided by the verdict |
| `NotPublished` | The adapter publishes no evidence | Not held by this pass |
| `Failed` | The read was refused, timed out or the adapter failed | Held until a later read of that venue succeeds |

A failed read says nothing about what the venue holds, so it is never read as agreement. A backtest reconciles nothing: it opens no venue.
A paper run reconciles against its simulated venues on every pass. The readiness detail carries one clause per venue, and the operator
console lists the latest passes with their findings.

A resume is refused while any order is unknown or reconciling, while any venue's latest evidence
read failed, and while any venue's latest verdict is `Diverged`. Only an `Aligned` pass clears a divergence: an `Unverified` pass has not shown
that the divergence is gone.

## Fees

A fill's fee is recorded in the asset and scale the venue charged it in, signed (positive paid,
negative rebate), and never netted across assets. A fill whose fee the venue did not state still
moves the position, because the venue said it executed and leaving executed quantity out would
understate exposure. The position counts such fills (`fills_without_fee`); while the count is
above zero its fee totals are a lower bound, never the cost.

## Position adjustment

A position the venue holds and no fill explains (held before the platform, a transfer) is accepted
into the record only by a risk officer's `adjust-position` command: venue, instrument, side,
scaled quantity, scaled price and a reason. The order path checks it against that venue's ledger,
then writes `PositionAdjusted` (`PositionAdjusted` class) naming the command, the actor and the
reason, then the command's outcome. The command's identifier is the change's identity, so the same
command issued twice moves the position once. A refused adjustment (no reason, a quantity or price
that is not positive, a change the ledger refuses) writes no adjustment and records the refusal.

An adjustment does not clear a divergence by itself: the next reconciliation pass that aligns does.

## Reconciliation while unsettled

After start, a live node reconciles again, at most once a minute, every venue where a pass could
still settle something: an order there is unknown or reconciling, its last pass diverged, or its
last read failed. The
pass runs after the commands of the same decision pass, so an adjustment just carried out is in
the ledger it reads. A venue that is due and not open, or whose adapter faulted, is reported in
the readiness detail rather than skipped.
