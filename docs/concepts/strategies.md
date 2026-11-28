# Strategies

A strategy is a separate program. It reads market events on standard input and writes proposals on
standard output. It holds no credential, opens no venue connection, and reaches the order path only
by emitting a proposal the control engine then judges. It is started as an executable file, so a
script begins with its interpreter line (`#!/usr/bin/env python3`), as `bijective-backtest --new`
writes one; without it the run ends at once.

The same program runs in a bounded backtest, in a paper session and in a live session. They differ
only in where the events come from and which clock drives them; the modes are described in
[Architecture](architecture.md).

## The protocol

Standard input carries one JSON object per line, tagged by `event`. Standard output carries one per
line, tagged by `emit`. A message is a whole line.

```
->  {"event":"book","instrument":"btc-perp","venue_ts_ms":"1788000000000","bid_price":"2654375",...}
->  {"event":"end_of_window"}
<-  {"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1500",...}
```

- **Scaled figures are strings.** A JSON number is a double in most parsers, and a scaled amount that
  lost its low digits is not the amount anyone wrote. A scaled figure sent as a JSON number is
  refused. Counts, such as the number of book levels, stay numbers.
- **A market order must carry a price cap.** Its cost is otherwise unbounded at submission.
- **A line the supervisor cannot read is refused and reported with its line number**, never
  salvaged. The lines after it are still read. A blank line is not a refusal.
- **Strict outwards, tolerant inwards.** An event the strategy's library does not know arrives as
  `Unrecognised`, so a strategy keeps running when the platform adds an event. A proposal the
  supervisor does not know is refused.
- An absent sequence number is absent, never zero.
- **Order terms.** A proposal may add `"time_in_force"` (`gtc` when absent, `ioc`, `fok`, `day`,
  `at_the_open`, `at_the_close`) and `"post_only": true`. A post-only proposal needs a limit price
  and a time in force that lets it rest. A venue that cannot honour the terms refuses the order
  unsent and says which term; nothing is sent under a near one (decision 0009).
- **Stops and exits.** `"stop_price_scaled"` makes a proposal a stop held at the venue: a buy when
  the price rises to it, a sell when it falls to it, then a limit order at `price_scaled` or a
  market order bounded by `cap_scaled`. `"reduce_only": true` makes an order that only reduces what
  is held, such as a protective stop; a halted node leaves it working (decision 0012).
- **Linked orders.** `"parent"` names the `client_id` of an earlier proposal: the proposal is
  held until that one's orders fill, then placed for the quantity filled, and must be reduce-only.
  `"oco"` names a group under the same parent: once one member's order fills, the others are
  cancelled. A bracket is an entry and two reduce-only children, a stop and a take-profit, in one
  group (decision 0013).
- **Expiry.** `"expire_at_ns"` (UTC nanoseconds) has the node cancel the order if it still works
  then; it needs a time in force that rests, and a proposal read after its expiry is refused
  (decision 0014).
- **Facts besides the top of book.** A run or session that names a venue's facts stream
  (`--market facts-<venue>`) is also sent `fact` events: trades, liquidations, funding rates, open
  interest, mark and index prices and long/short ratios, one event whose `kind` names which, with
  the kind's own fields beside the instrument, venue, `exchange_ts_ns` and `received_at_ns`. Its
  `fact_position` is a position on the facts stream, never one a decision names. A kind the
  platform adds later arrives as a `fact` too, so a strategy passes over what it does not use.
- **A top carries two instants.** `venue_ts_ms` is the venue's own stamp. `received_at_ns` is this
  node's clock when the top arrived, one clock for every venue, so it orders tops across venues as
  they could have been known. It is absent on a top recorded before receive times were kept.
- **A session opens with its venue.** Before the first top a strategy is sent `venue`: the venue
  the market stream belongs to and the fees declared for it (`taker_fee_bps_scaled`,
  `maker_fee_bps_scaled`, basis points at the platform's 8-decimal scale, with `fee_source`).
  Fees are absent where no declaration stands, which is not a fee of zero. It also carries
  `instruments`: each instrument the venue lists with its scales, price tick, quantity lot, minimum
  size, minimum notional, whether it is trading, its `kind` (`spot`, `perpetual`, `outcome`, `equity`, as the venue
  profile names it), its quote and settlement currencies and its `base_currency` (the asset a
  quantity is counted in, absent where the profile does not declare it), as the data engine declared them from
  the venue profile, so a price can be placed on the venue's increments before it is proposed. It
  asks no answer.
- **A strategy is told what became of its proposals.** Each answer the control engine
  records (refused, submitted, accepted, rejected, unknown, filled, cancel requested, canceled,
  expired, reconciling) reaches the strategy that proposed it as `order`, naming the proposal by
  its position in the proposal stream, with the identifier the strategy proposed under (the venue
  is sent one the node assigns, see [Order path](order_path.md)), a reason where there is one,
  and a fill's identifier, quantity and price at the venue's scales, and `recorded_at_ns`, so the
  time from `submitted` to the venue's answer is that venue's latency as this node saw it.
  `unknown` is neither placed
  nor failed. A live session hears the live node and a paper session the paper node, whose orders
  fill on the simulated venue. A strategy is told about the proposals it made in the current
  session.
- **A proposal carries the strategy that made it.** The supervisor writes the name it runs the
  strategy under onto every proposal (`--name`, or the program's file name without its
  extension), so a `strategy:<name>` limit binds it. What a strategy writes cannot set or change
  it. See [Risk and limits](risk_and_limits.md).
- **Exchange-neutral, not exchange-blind.** The instrument is always the platform's identity,
  never a venue symbol, so one strategy runs on any venue. A book top also carries `venue`, the
  venue it was quoted on, and a proposal may carry `venue`, the venue it was decided for. A
  decision that does not depend on where leaves it out and the desk's venue preference decides.
  A decision that does (an arbitrage leg, a venue's fee or depth) names it, and is executed there
  or refused, never moved. See [Risk and limits](risk_and_limits.md).

The protocol is defined by `bijective-strategy-engine::protocol`. Any language that reads and writes lines
of JSON can implement a strategy.

## The Python library

`bijective-strategy` is a standard-library-only convenience over the protocol. It defines no rules
of its own.

```python
from bijective_strategy import Book, EndOfWindow, answered, events, propose

for event in events():
    if isinstance(event, Book):
        if int(event.ask_price) - int(event.bid_price) < 200:
            propose(desk="alpha", instrument=event.instrument, side="buy",
                    quantity_scaled="1", price_scaled=event.bid_price,
                    client_id="c1", decided_at_ns="1")
        answered()
    elif isinstance(event, EndOfWindow):
        break
```

`answered()` tells the supervisor the strategy has finished with the current event. Under the
stepwise exchange the supervisor waits for it before sending the next one; a strategy that never
answers reaches its time bound and is cut short.

## The supervisor

`bijective-strategy-engine` spawns and supervises strategy runs.

- **It holds no credential and opens no venue connection.** This is a property of what it links,
  checked by the `Agent environment` gate.
- **Ready means a run could be bounded.** It probes the host for a writable, delegated cgroup v2
  subtree and refuses to start without one. The cgroup root is a required argument.
- Everything crosses the supervisor in both directions. A strategy never touches the store.

## Runs

A run is identified by a digest over its specification: program, arguments and environment, folded
in a fixed order. Two runs with the same specification are the same experiment.

- **Classes are types.** `Research`, `Observer`, `Auditor` and `Live`. A collector for an observer
  run has no method that accepts a proposal, so the restriction is a compile error rather than a
  check.
- **A research window is a position in the market stream, never a time.** A research run with no
  declared end position is refused rather than widened to the present.
- **A run with no ledger entry did not happen.** Recording a run returns a token, and admitting a
  bounded run's output requires it. Only a clean completion produces usable output; a run cut short
  by its time or memory bound does not.
- **Reproducibility is reported, not enforced.** A replay that produces different bytes is a run
  whose verdict cannot be stood behind, reported as such.

## Sets

A bounded run may name several strategies (`--strategy` repeated, or a directory given to
`bijective-backtest`). The window is read and decoded once and every strategy is sent the same
bytes, so the runs differ only in the program.

- Each strategy keeps its own sandbox, cgroup, ledger entry and attribution, exactly as a run of
  its own. A strategy that fails is reported on its own line and stops no other.
- Runs execute in parallel, at most one per core. Their results are recorded in the order the
  strategies were given, so the record does not depend on which finished first.
- Each strategy in a set is named by its file, so moving the set keeps each history.
- The same program given twice is refused: it is one experiment counted twice.
- A live or paper session may also be a set: one read of the market per poll, every strategy
  fed in the order given, and the session as the one writer of the stage's proposal stream.
  Each strategy keeps its own sandbox, session marks and record; one that fails is ended and
  recorded on its own while the others continue.

Measured with 16 strategies over a window of 20,000 book updates on 12 cores (release build):
9.9 s as one set against 56.3 s as 16 separate runs.

## Several streams

A run or session may read several streams (`--market` repeated; a bounded run gives each its own
`--to-position`): each venue's market stream, `market-<venue>`, and its facts stream,
`facts-<venue>`. Nothing in this is specific to one kind of strategy: an arbitrage, a portfolio
across venues, a basis or funding strategy, or a signal read on one venue and traded on another
all use it, and a strategy that reads one market is sent exactly what it was before. Every run
reads at least one market stream, since decisions are placed on, and measured against, its tops.

- Every market's opening lines are sent first, in the order the markets were named: in a live or
  paper session a `resumed` line naming the venue and that market's own resume position and gap,
  then its `venue` line. The events then arrive as one sequence: each stream keeps its own order,
  and across streams the next event is the one that arrived first on this node's clock. At one
  instant a fact goes before the top that came in the same frame, as the data engine recorded
  them, and then the order the streams were named decides. A top recorded before receive times were kept is placed by
  its venue stamp, which across venues only approximates arrival. The same markets and ends always
  send the same bytes.
- A live or paper session merges what each poll read. A top that reached the store after a poll
  goes out in the next one, so across venues a session's order can differ from a backtest's only
  among tops that arrived within one poll interval of each other.
- A position belongs to a venue's market stream, so every proposal names the venue it is for, and
  a stated `decided_at_position` is checked against what that venue had been sent. A proposal that
  names no venue, or a venue the run was not sent, is refused.
- The markets and their ends are part of the run's identity, and each proposal is attributed to
  the market of the venue it names, so a measurement prices it against that venue's book.
- Each market keeps its own session marks, so a restart reports the gap on each market
  separately. A session over several markets is a different experiment from one over any of them,
  so the markets are part of its identity.
- One market stream and one facts stream per venue; a second of either is refused.

## Live sessions

Without an end position, the same command starts a live session.

- A live session resumes at the present. Its first line is `resumed`, naming the market position
  the following events start from and what the strategy missed since its last session.
- The supervisor records how far each session was sent through the market stream every 256 events
  and when the session closes. A crash between marks makes the next reported gap larger, never
  smaller.
- Only a live session writes the proposal stream the control engine reads. The `Proposal streams`
  gate enforces this.

## The sandbox

Every run is bounded by `bijective-sandbox`, using Linux namespaces and cgroup v2 and no container
runtime:

- **No network.** The run executes in an empty network namespace, not behind a firewall rule. The
  test aims a connection at a listener on the host's own loopback and requires a second, unsandboxed
  run of the same probe to succeed, so the refusal cannot pass by accident.
- **Memory bound.** A run that exceeds its memory limit is killed by the kernel and recorded.
- **Time bound.** The supervisor owns the clock. A run that hangs without consuming CPU is still
  ended, and ending a run ends its whole process group.
- **No credentials** are reachable from inside a run.
- **Refuses rather than degrades.** Each missing precondition is a distinct refusal, detected by
  attempting the operation rather than by reading a setting.
- No `unsafe` and no libc bindings: namespaces are entered by executing `unshare(1)` and limits are
  plain writes under `/sys/fs/cgroup`.

The supervisor must itself run inside the delegated subtree. In production its systemd unit carries
`Delegate=yes`, which the deployment kit emits. Its start-up probe moves a throwaway process into
a cgroup it creates, as it moves every run, so a supervisor started outside the subtree refuses to
start with the reason instead of starting and ending every run with exit 111. See
[Bound a strategy run](../how_to/verify_an_agent_run_can_be_bounded.md).
