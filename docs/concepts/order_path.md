# Order Path

The control engine holds the platform's single trading state and the only route from a decision to
a venue. A node replays its limit state, verifies it, and comes up `Halted`.

## Clearance

`clear_to_submit` turns a risk evaluation into a `Clearance`, and it is the only place in the
platform that mints one. `VenueExecution::submit` accepts nothing else, so code that has not
evaluated cannot call it.

A clearance requires all three of: nothing breached, nothing left unjudged, and inputs within
the declared staleness bound. Not knowing whether the platform is within its limits is not the same as being
safe.

Judging belongs to the risk crates and acting on the judgement belongs to the control engine. The
minting lives in the control engine because the evaluation crate is kept free of dependencies.

## Placing an order

`OrderPath::place` is the only caller of `submit`. Inside it:

1. A halted platform is refused before limit state is consulted. A halt already decided is not
   re-derived from figures.
2. The decision is evaluated and, if it clears, the clearance is passed straight to `submit`. The
   clearance never leaves the order path.
3. **The intent is written before the venue call** and the settlement after it. A crash in between
   leaves an intent with no settlement, which reconciliation finds. The reverse order would leave
   capital moved with nothing on disk saying so.

What each finding does:

| Finding | Order | Platform |
|---|---|---|
| Breach | Refused | Safe state driven, one record per breach, read back |
| Unjudged limit | Refused | Halted |
| Stale figures | Refused | Unchanged |
| Clock skew | Refused | Unchanged |

Staleness stops the order and not the platform: whether missing inputs should halt trading is the
pre-trade check's decision against its declared bound, and two components acting on one condition with
different bounds would produce halts that cannot be attributed to a single cause.

Cancel requires no clearance. A cancel only reduces exposure, and gating it would keep a platform
in breach from getting out of a position.

## Resting orders when the node is not active

A halt refuses new orders and does not touch the ones already at a venue: a resting limit order can
still fill and grow exposure. At the end of every pass, while the node is `Halted` or `Reducing`,
the engine asks the venue to cancel each working order (accepted or part filled) that is not an
exit. What counts as an exit depends on what the node knows about the venue:

| Venue state | Reduce-only order | Any other working order |
|---|---|---|
| Read, agrees with the record, enforces reduce-only (placement precondition or fill-time clamp) | Left: an exit or a protective stop | Withdrawn |
| Reduce-only not enforced, or no guarantee declared | Withdrawn | Withdrawn |
| Latest evidence unread, or reconciliation diverged | Withdrawn: the record cannot vouch for what the venue holds | Withdrawn |

A cancel already asked for is pending and is not asked again. Its answer does not close the order:
only the venue's later evidence does. A cancel the venue rejects leaves the order working, so the
next pass asks again. An order on a venue the node cannot reach is counted and reported, not
skipped silently. Each cancel is recorded as `Withdrawn` in the control stream.

An operator's `cancel-orders` command, once carried out, names every order not yet finished at
that moment, exits and protective stops included, and the sweep withdraws each of them as it
becomes working, whatever the trading state. It changes no state and does not stop new orders;
a halt does that. The orders it names are read from the control stream, so a restart still
withdraws them. It is open to an operator and a risk officer, not an observer: it also removes
protective orders.

An operator's `flatten` command ([decision 0010](../decisions/0010-price-collar-and-flatten.md),
amended by [0012](../decisions/0012-stop-orders.md)) moves the node to reducing, orders every open
order cancelled, stops and exits included, and on each pass after the sweep places one
reduce-only market order per open position in the record for what working orders do not close.
On a venue that enforces reduce-only an order ordered cancelled is not counted, so the whole
position is closed at once. The orders are cleared by the signed command, not by the figures, so
a breach or stale figures do not stop them; each is judged on the record, the venue's rules and
its order rate, and written as an intent first. A venue whose evidence read failed or diverged is
reported, not flattened. The command stays in force until a resume. The sweep leaves the
flatten's own closing orders working on a trusted venue, whether or not it enforces reduce-only,
so a market order that waits for its fill is not cancelled and placed again on every pass.

A strategy's reduce-only proposal ([decision 0012](../decisions/0012-stop-orders.md)), such as a
protective stop, is judged like any reduce-only order and is an exit in the table above: a halted
node leaves it working where the venue enforces reduce-only.

A linked proposal ([decision 0013](../decisions/0013-linked-orders.md)) is admitted by its desk and
recorded as `OrderHeld`. Each pass, after the new proposals, releases what its parent's fills
made due through the same collar, reduce-only check and risk, under a new client order id; a
release is counted from the intents on the record, so a restart releases nothing twice. Nothing is
released while the trading state does not admit it or a flatten is in force. When a member of a
one-cancels-other group fills, the sweep withdraws the other members' orders, and a reduce-only
check does not count a member's peers against it. On a venue that does not enforce reduce-only, a
group's limit leg without a stop is held until the mark reaches its price.

An order with an expiry ([decision 0014](../decisions/0014-expiry-at-a-time.md)) is withdrawn by the
sweep on the first pass at or after it, whatever the trading state; the venue holds it as good
till cancelled.

## Idempotency and unknown outcomes

Every order carries a client order identifier assigned before the network call and kept across any
retry. The node assigns it at admission, `bl-<instant>-<position>`: the instant on this node's clock
and the proposal's position in the stream, both in base 36, at most 30 characters. It is written
in the intent before the submit, so a retry or a restart reads it back. A strategy's own identifier
is kept as the decision's identifier and is what the strategy is told back; it never reaches a
venue, because a venue refuses an identifier it has held before and a strategy's is unique only
within its own sessions. A venue answer that cannot be read, a timeout or a dropped connection is `Unknown`: the
order may have reached the venue. An unknown is never resubmitted under a new identifier and never
treated as a rejection; only the venue's own account can settle it, so it is left to
[reconciliation](reconciliation.md). A duplicate reported by the venue confirms the first attempt
landed.

## Budget

The order path enforces a spend; it never computes one. A `Decision` carries its `Spend`, computed
at admission from the instrument's precision rules.

- Only gross exposure is drawn, and only by orders that add exposure. A reduce-only order draws
  nothing, so risk-reducing orders are not refused when the platform is near its limit.
- A reduce-only order is first judged on the venue's ledger: it must oppose the held position and,
  with the reduce-only orders already working the same way, not pass through flat. Otherwise it is
  refused (`DoesNotReduce`, with the held and working figures) before risk, budget or the venue.
- **A sell must be covered where a short cannot be held.** A sell of a spot asset or an outcome
  share beyond what the record holds on that venue, less the sells already working there, is
  refused (`Uncovered`, with both figures) before risk, budget or the venue. The record is the
  bound: a holding the platform did not place is sold only after a risk officer adjusts it in.
- Each venue's submits are bounded by its configured order rate (`order_rate`: at most `orders`
  in any window of `per_ns`; required, no default). A submit the window has no room for is
  refused (`RateLimited`, with the wait) before any budget is drawn. Every submit sent is counted,
  whatever the answer, and a venue's own rate refusal closes the window for the interval it
  stated, or one window. A restart starts the window empty.
- Every binding scope is checked before any is drawn. A scope short of the spend refuses the whole
  order and draws nothing anywhere.
- A rejection that leaves no order releases the reservation. An unknown outcome does not, because
  the order may have filled, and neither does a duplicate, because the venue already holds the
  order under that identifier.
- A limit event is checked against the replayed state before it is written, so an event the
  replay would refuse never reaches the stream.
- A finished order (filled, cancelled, expired, rejected) keeps reserved only the filled share of
  what it took, rounded up; the rest is released under its decision. The release is derived, not
  tracked: `OrderPath::settle_reservations` compares every finished order with what its decision
  still holds, runs at every start and after every reconciliation pass, and releases nothing twice,
  so a crash between an order finishing and its release is repaired at the next start. A release
  is never larger than what the scope holds in total. Once the order's instrument is flat on its
  venue and no order there can still execute, the filled share is released too
  (`PositionClosed`). The sweep also runs after every recorded fill and adjustment, and is linear
  in the number of orders.

## The decision loop

Strategies write proposals to a stream; the control engine reads it and acts on each proposal in
order, each judged against limit state as it stands after the ones before it.

- **A restart resumes; it does not replay.** The highest proposal position already answered is a
  projection over the control engine's own stream, moved as the path writes. A refused proposal
  advances the mark as an intent does.
- **A proposal this build cannot read is refused and recorded**, never skipped. The proposals after
  it are still acted on.
- An admission refusal is carried in the desk register's own words.

## Operator commands

Commands arrive on their own stream and are applied before pending proposals, so a halt that
arrived while proposals were waiting takes effect before they are judged.

- The state recorded is the state read back, not the state requested.
- A refused command is recorded exactly as an applied one is, with the actor, role and reason.
- A resume is not re-applied after a restart. A node always comes back halted.
- A resume is refused while any order is unknown or reconciling, and the refusal names the count.
- A resume is refused while any venue's latest reconciliation diverged or its evidence read failed,
  and the refusal names the venues.

```
CommandApplied { command_id: "cmd-0", actor: "night-desk", role: Observer, action: Resume,
                 state: Halted, detail: "observer may not resume" }
CommandApplied { command_id: "cmd-1", actor: "duty-lead", role: Operator, action: Resume,
                 state: Active, detail: "" }
Refused { decision_id: "ord1", reason: Stale { .. } }
```

## Reaching a venue

The node configuration declares, per venue, the endpoint, the profile, the names of the credential
entries, the settle coin, the scales and a request timeout.

- Credentials are named, never carried. They are resolved through the secrets broker.
- All configured venues open, or none does.
- Every configuration error is refused at start, before the store is opened.

## Start-up cost

The control engine rebuilds its state by replaying the control and limit streams, then sweeps
finished orders for reservations to return. Both are linear in the number of events. Measured
2026-11-04 on a release build of this repository's development host:

| Events replayed | Orders | Open time |
|---|---|---|
| 200,000 control | 100,000 | 0.98 s |
| 200,000 control and 200,001 limit | 100,000 finished | 1.70 s |

That is about 200,000 to 235,000 events per second, so start-up stays under ten seconds up to
about two million events. Appends are bound by one sync each (about 5,800 per second on the same
host). Snapshot anchors, which would bound start-up independently of history, are not built; they
are due before a node's control stream approaches two million events.
