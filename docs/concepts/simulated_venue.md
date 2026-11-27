# Simulated Venue

`bijective-venue-simulated` is a venue double that fills. It implements the execution contract, so
the order path cannot tell it from a live adapter, and it matches against the recorded book under
declared models, so a research or paper run produces fills, positions and reconciliation through
the same code as a live run. It opens nothing and reads no clock: every instant is the caller's.

## One step

For each market input, in this order:

1. **The market moves.** The record updates the book, and orders already resting are matched
   against it.
2. **The strategy acts.** The caller delivers the input to the strategy, which may submit or cancel.
3. **Commands settle.** `settle(now)` applies every command whose latency has elapsed, in the order
   sent, and matches newly placed orders against the current book.

A resting order therefore always sees new market state before an order submitted in reaction to
it, and an order cannot fill against the market move that prompted it.

## Matching rules

| Rule | Behaviour |
|---|---|
| Arriving order | Takes the opposite side best first, at each level's own price, up to its limit; a market order takes any level |
| Remainder | A limit order rests; a market order's remainder is canceled, not left resting |
| Resting order | Fills at its own price when the opposite side crosses it under the fill model, up to the quantity on the crossing levels |
| Consumed liquidity | What a simulated fill took is unavailable until the recorded book restates that level; one displayed quantity is never filled twice |
| Cancel | Takes effect after the cancel latency; a fill before then counts |
| Validation | Unknown or not-trading instrument, quantity below the minimum or off the lot, price off the tick, or a repeated client identifier is refused, and nothing is placed |

## Declared models

| Model | Values | Meaning |
|---|---|---|
| Fill | `TradeThrough` (default) | A resting order fills only when the opposite side trades through its limit. Its place in the queue is unknown, so reaching its price is not evidence of a fill |
| | `Touch` | Fills when the opposite side reaches the limit; optimistic |
| Latency | submit and cancel, in nanoseconds | How long a command takes to reach the matching engine |

`SimulationModel::describe` states the matching version and the models in one sentence, so every
result names its execution assumptions. Fees are not charged yet; the description says so.

## Evidence and parity

The simulated venue keeps its own orders through the platform's lifecycle, so a simulated order
obeys the same state machine as a live one. `evidence()` reports what it holds as venue evidence:
every order, every fill and an explicit position for every instrument it traded. A platform ledger
built from the fills it reported reconciles `Aligned` with that evidence; a missed fill is found as
a divergence. The recorded Bybit run drives it in
`crates/adapters/bybit/tests/simulated_execution_on_recorded_traffic.rs`: orders placed into the
recorded book fill, the run is identical when repeated, and it reconciles.

`account()` shows positions as a venue displays them: the entry price is the open cost divided by
the quantity, truncated toward zero. The exact cost stays in the lots.

## Paper mode

The control engine's paper mode (`--mode paper`) opens one simulated venue per configured venue,
from its profile alone: no credential is read and no order leaves the node.

- Every book update the node reads from a venue's market stream moves that venue's simulated book.
  An order settles in the pass that placed it: with no latency modelled it meets the book it was
  decided on, then every later update.
- Fills reach the record the way a live venue's do: reconciliation reads the simulated venue's
  evidence and records each fill once. A paper node reconciles on every pass, because the simulated
  venue is local and costs no venue a request.
- **The sandbox account** ([decision 0003](../decisions/0003-sandbox.md)). Each venue starts
  with the balances its configuration declares (`"sandbox": { "<venue>": { "<currency>": "<money at eight decimals>" } }`); a venue without them is withdrawn, never given a default. A
  buy the account cannot pay for, fee included, is rejected (`InsufficientMargin`), an open buy
  reserves what it may cost, and every fill moves cash and carries the fee charged at the venue's
  declared maker or taker rate. A venue with no declared fee charges none.
- **A restart continues the run** ([decision 0018](../decisions/0018-paper-venue-rebuilt-from-its-record.md)).
  Before the first evidence read, each simulated venue takes back what the paper record says it
  held: every order it accepted under the same identifiers, its fills, the positions and cash
  they made, and its working orders resting again; a cancel the record asked for is carried out.
  The book is rebuilt from the market read next. A venue the record never traded on starts with
  its declared balances.
- The model is stated at start: trade-through fills, no latency modelled, and each venue's
  balances and fees.
- With no operator registry the node resumes itself by a recorded command. A run ends after
  `--drill-seconds` or on a stop signal (SIGTERM, an interrupt), between passes: it reconciles once
  more, so the record holds every fill the simulated venues made, then prints orders, fills,
  holdings, refusals, cash and equity. A live run stops between passes the same way.
- `bijective-paper` runs the recorder, a paper session and the node in one command
  ([Run a first strategy on a live market](../how_to/run_a_first_strategy_on_a_live_market.md)).
- `scripts/verify/paper_path.sh` drives a strategy end to end through it: a paper session proposes,
  a signed command resumes the node, the order is admitted against portfolio, desk and strategy
  limits, fills, is recorded, and the strategy is told it filled.

## Backtest mode

A backtest (`--mode backtest`, [decision 0015](../decisions/0015-backtest-through-the-order-path.md))
opens the same simulated venues as paper, from the same configuration, over a recording.

- The recording moves the clock. A pass runs where a proposal was decided, once five seconds of
  recorded time have passed since the last pass, and at the window's end, and reads nothing
  recorded after its step.
- The node starts halted. With no operator registry it appends one resume command to its own
  command stream, which the first pass carries out.
- At the end it prints orders, fills, positions, refusals, cash, the change in equity and the
  largest drawdown per venue and currency, and says that the strategy was told nothing of its
  orders: feedback in a backtest is stage 2 of the decision.
- `bijective-backtest <strategy> --config <node configuration>` runs the strategy and then the
  order path in one command.
