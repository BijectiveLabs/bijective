# Architecture

The platform is a set of separate programs that share one append-only event store. Each program is
a process boundary chosen for failure isolation: a fault in one must not reach the component that
can halt trading.

## Dependency direction

```
Market data       (venue adapters -> data engine)
  |
Strategy          (strategy engine: produces proposals only)
  |
Risk              (control engine: limits, pre-trade check, clearance)
  |
Execution         (control engine: order path, budget, settlement)
  |
Venue adapter     (venue protocol translation)
  |
Venue
```

The direction is enforced, not documented:

- Execution depends on risk; risk never depends on execution. Submitting an order requires a
  `Clearance`, and a clearance is minted only by a risk evaluation that passed. No code path from a
  strategy to a venue skips it. The `Order path closed` gate checks this on every change.
- An engine (data, control, strategy) depends only on the core, on the adapters and on its own
  area, never on another engine. `scripts/ci/areas.txt` declares every component's area and the
  `Dependency direction` gate enforces it.
- Nothing above the adapter contract names a venue. Adding a venue means adding one adapter crate
  and registering it.
- Operational components depend on the core; the core never depends on them. With every
  operational program stopped, the core starts, ingests and runs. The `Boundary` gate drills this.

## Programs

| Program | Binary | Responsibility | Class |
|---|---|---|---|
| Data engine | `bijective-data-engine` | Connects to venues, normalises market data and records it | Core |
| Control engine | `bijective-control-engine` | Holds the risk evaluation, the pre-trade check, the single trading state and the order path | Core |
| Strategy engine | `bijective-strategy-engine` | Supervises sandboxed strategy runs; holds no credential and opens no venue connection | Core |
| Operational host | `bijective-operational-host` | Sweeps the event store and reports each segment's verification verdict | Operational |
| Venue assurance | `bijective-verify-venue` | A scheduled check, daily by default, that a venue still behaves as its profile declares | Operational |
| Console | `bijective-console` | The operator view; the only network-facing surface, bound to loopback, or HTTPS with client certificates | Surface |

Supporting tools: `bijective-backtest`, `bijective-paper`, `bijective-promote` and
`bijective-report` for strategy work. `bijective-profile` drafts a venue profile from the venue's
published rules and `bijective-import` reads a client's market history file. `bijective-deploy`
installs, `bijective` makes operator keys and issues signed commands, `bijective-secrets` holds
the keystore, `bijective-archive` and `bijective-journal` keep the market archive and the frame
journal, and `bijective-check-ordering` checks ordering across streams.

## The core

Every program links the same core crates, and none of them belongs to an engine:

- **Clock.** One abstraction with a wall-clock and a sequence-driven implementation. Every component
  reads time through it, which is what makes deterministic replay possible. See
  [Event sourcing](event_sourcing.md).
- **Event store.** Append-only, hash-chained segments, single writer per stream. Programs
  communicate through it.
- **Bus.** In-process messaging inside one program. Messages that cross a process boundary go
  through the event store, because a lost cross-process message cannot be told from one never sent.
- **Codec and signal layouts.** The fixed binary encoding of events and the message layouts
  programs exchange.
- **Secrets broker.** The only path from a component to a credential. See [Secrets](secrets.md).
- **Model.** Platform identities and domain types shared by every program.

## Ordering across programs

Each stream has exactly one writer, so the order within a stream is the writer's sequence number. Across streams,
causality is recorded by the component that knows it: a decision record names the positions of the
events it consumed. No global sequencer sits on the order path, and no ordering is inferred from
timestamps.

## Run modes

A strategy is promoted through three modes, in this order: backtest, paper, live. The order is
enforced by the promotion stages ([Evaluation](evaluation.md)), and every program that takes a
mode names the same three.

| Mode | What it is for | Market data | Clock | Orders |
|---|---|---|---|---|
| Backtest | A strategy and the order path over recorded history | Recorded | Driven by the recorded sequence | Matched on a simulated venue per configured venue ([decision 0015](../decisions/0015-backtest-through-the-order-path.md)); no venue is opened |
| Paper | A strategy against live data with no capital at risk | Real time | Wall clock | Matched on a simulated venue per configured venue, from its profile; no order leaves the node and no credential is read |
| Live | Trading | Real time | Wall clock | Sent to the venue. Capital at risk |

A run reaches a venue in exactly one case: live mode on live inputs. A test over every combination
the command line can express holds that.

The same three modes in each program:

| Mode | Control engine | Strategy engine | Promotion stage |
|---|---|---|---|
| Backtest | `--mode backtest --replay-from <ns> --market-store <dir> --market <stream>` (reads `proposals-backtest`) | A bounded run: `--to-position` (class `research`, proposals to `proposals-research`) | `backtest` |
| Paper | `--mode paper` (reads `proposals-paper`; orders matched on the simulated venue) | A session: `--stage paper` (proposals to `proposals-paper`) | `paper` |
| Live | `--mode live` | A session: `--stage live` (proposals to `proposals`) | `live` |

A paper node keeps its own record: `proposals-paper`, `control-paper`, `limits-paper` and
`commands-paper` (`bijective_serialization::signal::node`). A paper session's proposals, its
feedback and a paper command (`bijective issue --paper ...`) can never reach, resume or be read as
the live node's, even when both run on one store. A backtest keeps `proposals-backtest`,
`control-backtest`, `limits-backtest` and `commands-backtest` in a record directory of its own and
reads the recorded market from `--market-store`, which it never writes.

The console reads any of the three records through the same panels (`--mode`,
[decision 0017](../decisions/0017-one-console-for-every-run-mode.md)): research and trading are
the same views of different runs, and the run's mode is on every page.

The control engine requires `--mode`. There is no default: a default of live would start a run that
reaches venues because a flag was forgotten. The deployment manifest names `--mode live`.

### Replay is not a mode

A mode says what a run is for. Replay says where its inputs come from, and it is a property of every
run, because every decision is journaled with the positions of the events it consumed:

- At start, each program rebuilds its state by replaying its own journal. The readiness state
  `Replaying` is this.
- A backtest is fed from a recording. `--replay-from <ns>` gives the instant its clock starts at;
  the clock then moves only with the recorded sequence, so the same recording yields the same
  bytes. The `Replay parity` gate runs a backtest twice and compares the control streams.
- Replaying a paper or live run on its own journal is not built yet. `--replay-from` is refused
  for those modes, so a paper or live run is never started on a clock it was not given.

A backtest places its proposals on the [simulated venue](simulated_venue.md), with passes at the
positions the recording calls for ([decision 0015](../decisions/0015-backtest-through-the-order-path.md)).

## Starting halted

Every program passes `Starting`, `Replaying` and `Ready`, and the control engine comes up with the
trading state `Halted`. Trading resumes only by an authorised operator command after state has been
rebuilt and verified. An install, a restart and a crash therefore begin from the same safe
position. See [Order path](order_path.md). A paper run or a backtest given no operator registry
resumes itself by a recorded command, since nobody could sign one and neither places capital
([decision 0015](../decisions/0015-backtest-through-the-order-path.md)); a live node refuses to
start without a registry.

## One code path for every mode

A backtest, a paper session and a live session run the same strategy program through the same
supervisor. They differ only in the clock implementation and in where market events come from. See
[Strategies](strategies.md).

## Deployment shape

All programs are built, released and installed together and carry one version. The default is a
single host; programs communicate only through the store, so distribution is a deployment choice.
No outbound telemetry, no external assets, and the console binds loopback unless given HTTPS with client certificates. See
[Deployment](deployment.md).
