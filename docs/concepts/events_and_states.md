# Event Streams and Trading States

Every fact the platform acts on is an entry in a named stream of the event store
([Event sourcing](event_sourcing.md)). This page lists the streams, the control events that
record the order path, and the trading states the control engine holds.

## Streams

| Stream | Written by | Read by | Content |
| ------ | ---------- | ------- | ------- |
| `frames-<venue>` | `bijective-data-engine` | `bijective-data-engine --replay-store` | Every frame received, with its arrival time, and the session and continuity marks; written before the frame is read |
| `market-<venue>` | `bijective-data-engine` | strategy engine, evaluation | Normalised top-of-book updates for one venue |
| `facts-<venue>` | `bijective-data-engine` | strategy engine | Trades, liquidations, funding, open interest, mark and index prices and long/short ratios for one venue, under the platform's instrument identity; facts on undeclared symbols are counted and stay in the journal and archive |
| `link-<venue>` | `bijective-data-engine` | console | Venue connection state, written on change only |
| `proposals` | strategy engine, `live` stage | control engine, `check_ordering` | Proposals from live sessions; the only proposal stream the control engine reads |
| `proposals-paper` | strategy engine, `paper` stage | control engine in paper mode, evaluation | Proposals from paper sessions; matched on the simulated venue, never reaches a venue |
| `proposals-research` | strategy engine, bounded runs | evaluation | Proposals from bounded runs over recorded history |
| `strategy-runs` | strategy engine | evaluation, report | Run ledger: specification, exit, output digest, and where admitted proposals landed |
| `strategy-sessions` | strategy engine | strategy engine | Market position a live session was sent through on each market stream, so the next session can report the gap |
| `registrations` | `declare_claim` | evaluation | Claims declared before a run: hypothesis, decision rule, horizon, folds |
| `commands` | operator tooling | control engine, console | Operator commands with actor and role |
| `control` | control engine | console, `check_ordering` | Control events (below) |
| `control-feedback` | control engine | strategy engine | What became of each proposal, mirrored from `control` with the control position it mirrors; completed from `control` on start |
| `venue-instruments-<venue>` | data engine of that venue | strategy engine | Each venue listing: instrument scales, increments, minimums and trading status, written at start when they differ from the standing record |
| `limits` | control engine | control engine, console | Limit declarations and the consumption and release of budget |
| `control-paper`, `control-paper-feedback`, `limits-paper`, `commands-paper` | control engine in paper mode, operator tooling | strategy engine (paper stage) | The paper node's record, kept apart from the live node's |
| `venue-assurance` | `bijective-verify-venue` | fee declaration | Outcome of each venue check, with the digest of the answer it was drawn from |
| `venue-fees` | fee declaration | evaluation | Venue fees; accepted only on a confirmed check drawn from evidence |

A stream name admits only alphanumerics, `-` and `_`. Entries are append-only and addressed by
position; a reader resumes from the last position it acted on.

## Control events

The control engine writes one event per decision on the order path. Each event carries
`at_ns` (UTC nanoseconds, from the program clock) and, where a stream entry caused it,
`from_position` (the position of that entry).

| Event | Written when | Key fields |
| ----- | ------------ | ---------- |
| `Intent` | An order cleared every check, before the venue call | `decision_id`, `venue` (absent on intents recorded before layout version 4), `owner` (the desk and, where named, the strategy the order was placed for; absent before layout version 5), `request` (client order id, venue symbol, side, quantity and limit price in venue scale, reduce-only) |
| `InstrumentScales` | Before the first intent on an instrument, and again when its profile declares other scales, identity or quote currency; layout version 15 | `venue`, `instrument` (venue symbol), `price_scale`, `quantity_scale`: the decimals every scaled price and quantity of its orders and fills carries, so the record is read without the profile; `identity` (the platform's identity, the one market streams carry) and `quote_currency`, absent before layout version 17 |
| `Held` | A linked proposal was admitted and is held until its parent fills (decision 0013); layout version 12 | `proposal` (whole, with its `parent` and `oco`), `from_position` |
| `Settled` | The venue answered a placement | `client_id`, `outcome` |
| `Withdrawn` | A cancel was sent and the venue answered | `client_id`, `instrument`, `outcome` |
| `Filled` | A fill of an order this platform placed, as the venue reported it; recorded once per execution identifier | `client_id`, `fill` (identifier, venue order identifier, side, price, base quantity, fee, liquidity, exchange time, receive time) |
| `Refused` | An order did not reach the venue | `decision_id`, `reason` |
| `Captured` | A decision was benchmarked against the book at the strategy's decision instant | `correlation_id`, `decided_at_ns`, `book` (absent when none was within the staleness bound) |
| `CommandApplied` | An operator command was carried out or refused | `command_id`, `actor`, `role`, `action`, `state` read back, `detail` |
| `SafeStateDriven` | A safe state was called for | `target`, `reason`, `arrived` (read back), `scope` |
| `SafeStatePostcondition` | One pass after a safe state, exposure was compared | `scope`, `kind`, `before`, `outcome` (`Held`, `Increased` or `Unverifiable`) |
| `VenueParticipation` | A venue joined or left the routed set | `venue`, `participation` |
| `DayOpened` | The first complete valuation of a UTC day, when a daily-loss or drawdown limit is declared | `day` (days since the epoch), `unrealized_scaled` (money scale) |
| `DayPeak` | The day's result rose above its highest so far, when a drawdown limit is declared | `day`, `result_scaled` (money scale) |
| `AccountCharged` | A venue charged the account without naming an order (a daily regulatory fee), recorded once per charge identifier; layout version 9 | `venue`, `charge` (identifier, asset, amount paid, scale) |
| `ScopesValued` | The figures below the portfolio changed by the same rule as `PortfolioValued` | `figures`: each scope (desk, venue, strategy, underlying, instrument), kind and value (money scale; net signed). A scope the pass could not measure is absent. The console's Exposure by scope panel reads it |
| `PortfolioValued` | The portfolio figures changed by a hundredth, a figure appeared or was lost, or a minute passed with any change | `figures` (gross and net exposure, daily loss, each absent when not computed; money scale; leverage and margin use at the ratio scale, present only where a limit declares them and margin was read; equity, what every venue states backs its account in the book's currency, a simulated venue's cash with its positions at the node's marks, absent when a venue is withdrawn or states none, layout version 16), `observed_at_ns` |

`Intent` is written before the venue is called, so a crash between the two leaves an intent that
reconciliation can resolve against the venue. A venue answer that leaves the order's fate open
(a timeout, for example) is recorded as such and resolved by reconciliation, never treated as a
fill or a failure.

### Refusal reasons

| Reason | Meaning |
| ------ | ------- |
| `NotActive` | The trading state does not admit new orders |
| `LimitsUnrecovered` | Limit state has not been rebuilt, so no budget figure exists |
| `Breached` | Observed exposure exceeds a limit; the action taken is in `SafeStateDriven` |
| `Unjudged` | A limit could not be evaluated |
| `Stale` | Exposure figures are older than the declared freshness bound |
| `ClockSkew` | Exposure figures are stamped after the instant they were judged at |
| `NoBudget` | A binding scope has insufficient remaining budget for the order's spend |
| `NotAdmitted` | Desk admission refused the proposal; the admission message is carried as text |
| `Unreadable` | The proposal could not be decoded by this build |
| `InstrumentRefused` | The venue does not take orders for the instrument, or it is below the venue minimum |

## Trading states

The control engine holds one trading state for the node.

| State | Effect on new orders |
| ----- | -------------------- |
| `Active` | Admitted, subject to the pre-trade check |
| `Reducing` | Refused when they extend an existing net position. A flat instrument is not constrained |
| `Halted` | Refused |

Severity is ordered `Active < Reducing < Halted`.

| Transition | Trigger |
| ---------- | ------- |
| start -> `Halted` | Every start. Limit state is replayed and verified first |
| any -> more severe | A breach, an unjudged limit, or a failed postcondition. Automated escalation never relaxes the state |
| any -> `Halted` / `Reducing` | Operator `Halt` / `Reduce`, when the role permits it |
| any -> `Active` | Operator `Resume` only, when the role permits it. Not re-applied after a restart |

A breach selects `Reducing` only for an instrument-level breach on a held position at a venue
whose reduce-only orders are a placement precondition. Every other breach selects `Halted`,
because a venue that clamps reduce-only at fill time would still fill against no position.

The state recorded in `CommandApplied` and `SafeStateDriven` is the state read back after the
change, not the state requested.
