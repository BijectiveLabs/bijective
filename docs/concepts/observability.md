# Observability

The platform must be able to answer, without anyone reading source during an incident, why an
order was sent, what the venue answered and how the position changed. Every consequential action
is an event in the store; the surfaces below read those events and never hold authoritative state.

## Readiness

Every program writes a readiness document that operators and the gates read.

- **Running is not ready.** The states are `Starting`, `Replaying`, `Ready` and `Failed`; there is no
  `Running`. An init system reporting a process alive is not evidence of anything.
- **Readiness is not the trading state.** A correctly halted control engine is `Ready`, and names
  the trading state in its detail line. A halted node must not look like one that failed to start.
- **Every way of not knowing reads as not ready**: an absent document, an unreadable one, one from
  an unknown build, and a stale one. Staleness is judged by the reader's clock, because a program
  that has died cannot update the field that would say so.
- The document is written to a sibling file and renamed, so a reader never sees a partial write.
  Its mode is `0644`: asking whether a node is up needs no privilege.
- A program whose first readiness report fails exits and says why. Later failures are warned on
  standard error.

## Missing-event detection

`bijective-detection` declares, per integration, what must produce at least one event within what
interval, and whether its subscription must be confirmed attached.

- **An undeclared integration is `UNMONITORED`, never healthy.** Zero events from an unmonitored
  integration cannot be distinguished from zero events from a healthy one.
- **Every expectation states the incident it came from.** An interval with no reason is an
  unexplained constant; an empty origin is refused.
- The registry is plain versioned JSON read with the standard library. A reference registry for a
  crypto venue integration ships in `python/bijective-detection/reference/` and is meant to be
  edited.
- Only absences belong in the registry. Too many events, wrong events and durability defects are
  handled by the components they concern.

## Ordering checks

`bijective-check-ordering` reports the recorded order of events
across the control and proposal streams of a node.

## Store verification

The operational host sweeps the event store and reports each segment's verdict (`Clean`, `Corrupt`,
`Unreadable`). It reports the verifier's verdict and never interprets it.

## The console

`bijective-console` is the operator view: one surface whose panels are grouped behind routes
(overview, market, data, venues, orders, risk, detections, audit), with a strip on every page that carries what needs
attention now: the node's standing, the trading state last recorded, links not connected, venues
withdrawn, reconciliation diverged or unread, orders whose fate is open, and entries no build could
read. The strip opens with the run mode whose record is shown (`--mode`, live unless given;
[decision 0017](../decisions/0017-one-console-for-every-run-mode.md)): a paper or backtest record
is read through the same panels and tables as the live one, and the control page signs for that
mode's node only. Every figure is a fold over the event streams; nothing it holds is authoritative, and losing
it degrades visibility without degrading the platform.

- It is the only network-facing program. It binds loopback, reached through the operator's own SSH
  tunnel, unless it is given HTTPS with a client authority: then any address, and a connection
  without a certificate that authority issued is refused in the handshake. See
  [Reach the console](../how_to/reach_the_console.md).
- It answers reads, and one form: the control page (`/control`), present when the console is
  started with an operator, signs halt, reduce, resume, cancel-orders, flatten and venue withdrawal or
  admission with that
  operator's key, as `bijective issue` does. Each form carries a token that changes every start;
  a form without it, or from another origin, signs nothing. The control engine verifies signature
  and role on arrival; the console decides nothing. `/metrics` answers the strip as OpenMetrics
  text (below), and `/api/` the panels as JSON (Tables for programs). Any other route that is not a group answers not found, and any other request that
  is not a read is refused.
- `/metrics` carries one gauge per strip label, `bijective_signal{label="..."}`, at 0 normal,
  1 attention or 2 alarm (the highest any panel gave that label), and `bijective_alarms`, the
  count at alarm. A halt, a divergence, evidence unread, an order whose fate is open or a limit
  that cannot be judged reaches an alert rule exactly as it reaches the page, for example
  `bijective_alarms > 0` for one minute. The console sends no notification itself: delivery is
  the scraper's alert manager's, reached over the same loopback tunnel or client certificate as
  the pages. A scrape is a read like any page, and refreshes the fold.
- It loads no external assets and its pages run no script: no CDN, no remote fonts, no telemetry.
  Pages refresh themselves every ten seconds.
- Given `--app <dir>`, it also serves a static web application from that directory under `/app/`
  (`index.html` at `/app/`). The application reads the same tables (below) and issues commands
  only through `/control`, so it reaches nothing a page cannot. Only plain names are served
  (letters, digits, `.`, `-`, `_` and `@`; no hidden file, parent step or link leading outside the
  directory, at most 32 MiB, and only `html`,
  `js`, `mjs`, `css`, `json`, `wasm`, `svg`, `png`, `woff2` and `txt`). Its policy lets it run its
  own scripts, workers and WebAssembly (including `blob:` scripts its own code makes, as a table
  engine does when it unpacks its WebAssembly) and connect to this console only; no inline script
  runs and nothing loads from a network. Any application can be served this way, from the operator's own to a vendor's.
- What it costs the node: nothing on the order path (it shares no lock, memory or thread with the
  engines) and CPU on the host. The console keeps its fold between requests: the first request
  reads the control, limit, command and link streams whole (about 0.8 s for 200,000 events), and
  each later one reads, verifies and decodes only what was appended since, from a cursor into the
  newest segment (about 0.2 ms when ten events were added). A stream found shorter than the
  cursor was replaced, and the fold starts over. A market stream is read from its newest 300
  entries only, never from its history. A source that fails to read is listed once among the
  unreadable entries, not on every request.
- Given `--archive <dir>`, the market page also reads the newest sealed files of each venue's market
  archive (at most 50,000 fact rows per venue, always the newest file): the last trade, mark, index,
  funding and open interest per instrument, and the newest 50 liquidations. Figures are shown at
  the scales the files declare. A file is sealed when a minute of received time closes, so these
  trail the venue by about a minute; each row shows its newest arrival. A venue whose archive
  cannot be read is counted with the unreadable entries.
- The venues page shows each venue's order latency: from the intent, written just before the
  venue call, to the settlement, written once the venue answered, so signing, the network both
  ways and the venue's processing. p50, p95 and the maximum over the newest 200 answers per venue,
  with the count still awaiting an answer. No latency bound is declared, so it informs and never
  raises the strip.
- The portfolio figures on the risk page and the strip (gross exposure, daily loss, and leverage
  and margin use where a limit declares them, against their limits, and the book's equity where
  every venue states it) are the ones the control engine recorded as `PortfolioValued`; the console
  computes none of them. A declared limit whose figure is missing is an alarm.
- The orders page lists the proposals the platform holds for a parent's fill or a group
  ([decision 0013](../decisions/0013-linked-orders.md)): parent, group, quantity, what has been
  released, and whether each is still held, released, displaced by its group or ended by an
  operator. It is folded with the node's own projections, so it reads a holding as the node does.
- The orders page also lists the newest 200 fills as the venues reported them (recorded once per
  execution identity), with the venue and instrument of the order each filled and its fee, signed:
  positive paid, negative a rebate. The risk page lists the account charges that name no order,
  such as funding, with the same sign. A fill whose intent is not in the stream names no venue
  rather than a guessed one.
- A panel owns its fold and its view. The console reads each stream once per request, decodes each
  entry once and offers it to every panel, so a panel is added as one file and one line in the list
  of groups, without changing another panel.
- Operator actions travel a separate, recorded and signed command stream, never the read path. See
  [Order path](order_path.md) and "Who is asking" in [Risk and limits](risk_and_limits.md).

### Tables for programs

The same fold that renders a page also answers as JSON, for a dashboard, a notebook or a script.
The format is open and versioned; nothing in it is reserved to a paid component.

| Route | Answers |
| ----- | ------- |
| `/api/panels` | The index: every table by `panel` name and `title` |
| `/api/panels/<panel>` | One table, named after its panel's title (below) |
| `/api/events` | Server-Sent Events: every table once, then each again when it changes |
| `/api/control` | What this console may sign: `issuing`, the run `mode`, and with an operator its `operator`, `role`, offered `actions`, known `venues` and the form `token`, for an application of this origin to post the Control page's own form to `/control`. Never on the event stream; a browser does not let another site read it, and `/control` refuses a post from another origin |

Every panel answers as a table except the control forms, which issue rather than report: `strip`,
`standing`, `top-of-book`, `trades--funding-and-open-interest`, `liquidations`, `links`,
`entries-this-build-could-not-read`, `venues`, `venue-latency`, `reconciliation`, `orders`,
`fills`, `held`, `refused`, `decisions`, `portfolio`, `exposure-by-scope`, `account-charges`,
`limits`, `safe-state`, `safe-state-honoured`, `operator-actions`, `open-detections`,
`detections`, `execution-cost`, `strategy-performance` and `strategy-performance-history`. The name is the title in
lower case with every other character a dash; `/api/panels` lists them.

- A table is one line of JSON: `format` (now 1, raised by any change that breaks a reader),
  `panel`, `title`, `columns` and `rows`. Each column has a `name`, a `unit` and a `scale`. Units
  are `text`, `flag`, `ns` (UTC nanoseconds), `money` (the book's settlement currency, scale 8),
  `ratio` (scale 4, so 25000 is 2.5), `count` (a whole number, scale 0), `span` (a duration in
  nanoseconds, scale 6, so it reads in milliseconds), `instrument` (the instrument's own price or
  quantity scale, which the row does not carry, so `scale` is `null`) and `decimal` (an exact
  decimal written out, such as `-0.0030`, at the scale its source declared for that row, so
  `scale` is `null`).
- Every integer is a decimal string, never a JSON number: a nanosecond time or a scaled amount is
  past what a JavaScript number keeps exact. Divide by ten to the `scale` once, for display only.
- In `orders` and `fills`, quantities and prices are decimals at the scales the record declared
  for the instrument on its venue (`InstrumentScales`); a record written before scales were
  declared carries `null` there rather than a guessed decimal. The page shows the scaled integers.
- An absent figure is `null`, never `0`. In `portfolio` and `exposure-by-scope`, a `null` figure
  beside a declared limit is unmeasured; beside a `null` limit it is not tracked.
- `portfolio` has one row per recorded summary, oldest first, the newest 240. Each row carries the
  limits in force when it was recorded, except the newest, which carries those in force now, as
  the page and the strip judge it. The history is the record's, so a reader that opens late sees
  the same past as one that was open throughout.
- Two folds of the same streams give the same bytes, so a table can be compared, hashed or stored.
- An event is named after its table and carries the whole table, sent again only when its bytes
  change; a pass with no change sends a comment line. Each event stream folds on its own thread,
  once a second, so the pages are never kept waiting by a reader. At most four are open at once;
  a fifth is answered 503 and opens nothing. A stream ends when its reader leaves, found within
  about two seconds.
- The routes are reads like any page: the same loopback or client certificate, the same security
  headers. They issue nothing; commands go through `/control` only.

### Detections

A detection is what a detector concluded about one subject: a venue answering slower than it did,
fills reaching the node late, rejections bunching. The platform's open core runs no calibrated
detector; it defines the format, reads the detections stream of its run mode (below), and shows it on the
detections page (open conclusions, and the conclusions as written) and on the strip (`detections`,
at alarm while any conclusion at alarm stands). Any program may write the stream, an operator's
own as well as a vendor's, and is shown the same way.

Each entry is one JSON object:

| Field | Value |
| ----- | ----- |
| `format` | `1`, raised by any change that breaks a reader |
| `detector` | The detector's name, not empty |
| `subject` | What it is about, `<kind>:<name>`, such as `venue:<name>` or `order:<client id>` |
| `level` | `attention`, `alarm`, or `cleared`, which withdraws the detector's standing conclusion on the subject |
| `detail` | One sentence an operator can act on |
| `measured`, `threshold` | Exact decimals written out (`"10.674"`), or `null` |
| `source` | The stream the evidence is in |
| `evidence` | Positions in `source`, as decimal strings, the last the entry concluded on |
| `at_ns` | When the evidence says it happened, UTC nanoseconds as a decimal string |

- An entry that does not parse to exactly this is listed with the unreadable entries, never
  shown in part. `bijective_console::detection::encode` writes the format for a Rust program.
- A writer concludes from the record, not from its own clock, so a replay of the same stream
  writes the same detections, and the evidence lets an operator check each one against it.
- The stream has one writer at a time, like every stream of the store.
- Detections are kept per run mode like the node's streams: `detections` about the live node,
  `detections-paper` and `detections-backtest` about the others; the console reads its mode's.

### Strategy performance

What a desk's strategy came to on one instrument: its fills, position, realized and unrealized
profit and loss, fees, net, drawdown, arrival cost and markouts. The open core computes none of
it. It defines the format, reads the performance stream of its run mode, and shows the newest
entry per subject and the entries as written on the strategies page. Any program may write the
stream, an operator's own as well as a vendor's, and is shown the same way.

Each entry is one JSON object:

| Field | Value |
| ----- | ----- |
| `format` | `1`, raised by any change that breaks a reader |
| `desk`, `strategy`, `venue`, `instrument` | The subject: the owner the intents named, and the venue's symbol; `strategy` may be `null` |
| `identity`, `quote_currency` | The platform's identity for the instrument and the currency money is stated in, as the record declared them, or `null` |
| `fills`, `wins`, `losses` | Counts: fills, and closing fills that realized a gain or a loss |
| `position`, `traded_notional`, `realized`, `fees` | Exact decimals written out |
| `unrealized`, `net`, `peak_net`, `drawdown`, `arrival_cost`, `arrival_cost_bps` | Exact decimals, or `null` where none could be made |
| `fees_excluded` | Assets a fee was charged in other than the quote currency, left out of `fees` and `net` |
| `markouts` | One object per horizon: `horizon_s`, `bps` (an exact decimal or `null`, positive in the fill's favour) and `fills` measured |
| `source`, `through` | The control stream folded and the last position of it the entry reflects |
| `at_ns` | The record's time the entry was concluded at, UTC nanoseconds as a decimal string |

- An entry that does not parse to exactly this is listed with the unreadable entries, never
  shown in part. `bijective_console::performance::encode` writes the format for a Rust program.
- Performance is kept per run mode: `performance` about the live node, `performance-paper` and
  `performance-backtest` about the others. The console reads every mode's stream in its store and
  the `performance-backtest` stream of each backtest record under `run/backtests/<window>`, and
  names each row's run (`live`, `paper`, `backtest <window>`), so one strategy's runs stand side by
  side (decision 0017 stage 2). Commands stay with the console's own mode.
- The markout horizons the console shows are 1, 5, 10, 30 and 60 seconds
  (`bijective_console::performance::MARKOUT_HORIZONS_S`); a writer measures at those.

