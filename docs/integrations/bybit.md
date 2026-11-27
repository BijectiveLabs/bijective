# Bybit

`bijective-venue-bybit` connects to Bybit V5 linear perpetuals.

| Capability | State |
|---|---|
| Public order book stream | Built and verified against the live venue |
| Signed REST account read | Built and verified against the live venue |
| Order submission and cancel | Built and exercised through a node 2026-11-19 and 2026-11-20: time in force, post-only and stop orders as described below |
| Amend | Not built |
| Venue evidence (open and recent orders, executions, positions over signed REST) | Built and read by a node against the venue: reconciliation stayed aligned through the stop orders of 2026-11-20, and pushed fills were compared with the listed ones on 2026-11-18 |
| Private stream (authentication, order and execution topics) | Built and run against the venue: prompts an evidence read when the venue reports a change, and supplies the fills it pushed before the execution history lists them |

## Sources

Read 2026-08-26 unless stated. A claim here is evidence about the date it was read; the scheduled
venue check (`scripts/verify/venue_assurance.sh`) is what confirms it today.

| Source | Document | URL |
|---|---|---|
| Intro | V5 API introduction | `https://bybit-exchange.github.io/docs/v5/intro` |
| Instruments | Instruments info | `https://bybit-exchange.github.io/docs/v5/market/instrument` |
| Create order | Create order | `https://bybit-exchange.github.io/docs/v5/order/create-order` |
| Orderbook | Public orderbook stream | `https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook` |
| Public trade | Public trade stream (read 2026-11-06) | `https://bybit-exchange.github.io/docs/v5/websocket/public/trade` |
| Ticker | Ticker stream (read 2026-11-06) | `https://bybit-exchange.github.io/docs/v5/websocket/public/ticker` |
| Liquidation | All liquidation stream (read 2026-11-06) | `https://bybit-exchange.github.io/docs/v5/websocket/public/all-liquidation` |
| Rate limits | Rate limits | `https://bybit-exchange.github.io/docs/v5/rate-limit` |
| Connect | WebSocket connect, authentication, heartbeat (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/ws/connect` |
| Private order | Order stream (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/websocket/private/order` |
| Private execution | Execution stream (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/websocket/private/execution` |
| Open orders | Get open and closed orders (read 2026-11-04, 2026-11-17) | `https://bybit-exchange.github.io/docs/v5/order/open-order` |
| Order history | Get order history (read 2026-11-17) | `https://bybit-exchange.github.io/docs/v5/order/order-list` |
| Executions | Get trade history (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/order/execution` |
| Positions | Get position info (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/position` |
| Enums | Enums definitions (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/enum` |
| Fee rate | Get fee rate (read 2026-11-04) | `https://bybit-exchange.github.io/docs/v5/account/fee-rate` |
| Instruments, live | Instruments info, live | `GET https://api.bybit.com/v5/market/instruments-info?category=linear&symbol=...` |

The specifics of order-placement fields and the orderbook stream's sequence and resubscribe semantics
are implemented against these documents but not yet written out here.

## Rate limits (Rate limits, read 2026-11-04)

- HTTP: 600 requests in a 5-second window per IP by default, across all traffic to the venue. The
  client reads with a quota of half that, 300 per 5 seconds; submits are bounded by the control
  engine's order rate.
- **HTTP 403 "access too frequent" means the IP has exceeded the limit.** The venue says to end all
  HTTP sessions and wait at least 10 minutes; the ban lifts automatically. The adapter reads a 403 as
  a rate limit with a 600-second wait, not as a refused credential (a refused key answers 401).
- The API limit is per second per UID, answered with `"retCode": 10006, "retMsg": "Too many visits!"`, and every answer carries `X-Bapi-Limit-Status`, `X-Bapi-Limit` and
  `X-Bapi-Limit-Reset-Timestamp`. The adapter does not read these yet; whether a `10006` arrives as
  HTTP 200 or 429 is to be observed.
## Order terms (Place Order, read 2026-11-19)

`timeInForce` follows the order's terms (decision 0009): left out for `gtc` (the venue's default),
`IOC`, `FOK`, and `PostOnly` for a post-only order with `gtc`. A market order always executes as
the venue's IOC, so `fok` and post-only on a market order are refused unsent, as are `day`,
`at_the_open` and `at_the_close`, which the venue does not offer. A post-only order that would
take is cancelled by the venue, not rejected; reconciliation records it cancelled. Seen
2026-11-19 through the node: an IOC buy and a FOK sell filled, a post-only buy at the ask was
cancelled, a post-only buy below the bid rested.

## Stop orders (Place Order, read 2026-11-20)

A stop (decision 0012) is a conditional order: `triggerPrice` at the instrument's price scale,
`triggerDirection` 1 for a buy (the price rises to it) and 2 for a sell (it falls to it),
`triggerBy` `LastPrice`; `orderType` `Market` for a stop-market, `Limit` with `price` for a
stop-limit. The venue lists it as `Untriggered` until it triggers, which reads as working, and
`Deactivated` when it is cancelled first, which reads as cancelled. It holds at most 10 conditional
orders per symbol. Seen 2026-11-20 through the node: a reduce-only stop-market sell and a buy
stop-limit rested `Untriggered`, reconciliation stayed aligned, and both were cancelled by
`flatten` with the position closed at the market.

## Return codes the adapter reads (Error codes, read 2026-11-05)

| Code | Venue text | Read as |
|---|---|---|
| `0` | success | accepted |
| `110072` | OrderLinkedID is duplicate | `Duplicate`: the client identifier is already at the venue. After a timeout this is the answer to a resubmission that confirms the first attempt landed; read as a rejection it would close an order the venue holds |
| `10006` | Too many visits. Exceeded the API Rate Limit | rate limit; reads wait one 5-second window |
| `10018` | Exceeded the IP Rate Limit | rate limit; reads wait one 5-second window |
| `110007` | ab not enough for new order (observed 2026-11-19 on an account holding nothing) | `InsufficientMargin` |
| `110094` | below the minimum order value | `BelowMinimumSize` |

Any other code travels as itself, unclassified, so a venue that changed is visible.

## Bounds on the connection

The connection is opened within 5 seconds (REST) or 15 seconds (WebSocket, the session's own
connect deadline), a body past 8 MiB is read as unreadable, and a WebSocket message or frame past
4 MiB ends the connection. None of these is a venue figure; they are the platform's.

- WebSocket: no more than 500 connections in 5 minutes per IP, and no more than 1,000 market-data
  connections per IP.

## Precision is per instrument (Instruments)

Read live for `BTCUSDT`, linear: `tickSize` 0.10, `qtyStep` 0.001, status `Trading`, settle and
quote coin `USDT`. Other instruments on the venue differ, which is why the profile carries
precision per instrument.

## Minimums (Instruments)

Read live on the same call: `minOrderQty` 0.001, `maxOrderQty` 1500.000, `minNotionalValue` 5. The
profile declares minimum size and minimum notional, the order path refuses orders below them, and
the venue check verifies both against the venue.

## Funding interval is published (Instruments)

`fundingInterval` is 480 minutes (`28800000000000` ns). The profile uses the `published` funding
model, so the interval is the venue's figure, not a platform constant.

## Order book stream

- **The sequence check is `seq > last`, never `seq == last + 1`.** The sequence is a per-connection
  counter, not a per-symbol one. With several symbols on one connection it advances by more than one
  between two deltas for the same symbol; requiring consecutive numbers would raise a gap on every
  healthy batch.
- A zero quantity removes a level.
- A delta before a snapshot, a repeated sequence or a reversed sequence invalidates the book. It is
  cleared, the stream is resubscribed, and continuity is `GapOfUnknownExtent`.
- A crossed book is reported, not acted on: one side going stale during a fast move can cross a book
  the next delta repairs.
- An unreadable level is refused with its side and index, never dropped. A message class the adapter
  does not handle is named, not skipped.
- The subscribe result arrives as its own message and is what confirms a subscription.
- At most ten topics per connection, and a ping inside thirty seconds or the venue drops the
  connection. The adapter pings every twenty seconds.

## Trades, ticker and liquidations

Every endpoint carries `orderbook.50.<symbol>` and `publicTrade.<symbol>`. A derivatives endpoint
(path containing `/linear` or `/inverse`) also carries `tickers.<symbol>` and
`allLiquidation.<symbol>`; spot has neither, and a topic the venue refuses would degrade the whole
connection. With four topics per instrument and ten per connection, one derivatives connection
carries two instruments.

- **Trade.** One record per execution. The side is the taker's, so `Buy` is a buyer aggressor. The
  venue's trade identity is kept; a repeated delivery is recognised by it.
- **Liquidation.** The venue states the side of the liquidated *position* (`Buy` is a long). The
  record states the side of the *forced order*, the opposite: a long is closed by a sell.
- **Ticker.** Snapshot first, then deltas, and a field absent from a delta has not changed. One
  record per field a message carries: open interest in base quantity, mark price, index price. A
  funding record needs the rate and the next funding time together; when a delta carries only one,
  the other is the last the venue stated, and with none stated no funding record is made.
- **Funding rate is kept as written.** The scale is the digits shown (`0.0001` is 1 at scale 4).
  A live frame on 2026-11-06 reads `0.00005671` with `fundingCap` `0.00333` and
  `fundingIntervalHour` `8`, and the REST ticker answered `0.00005727` for the same symbol: a
  fraction of the position per funding interval, not a percentage. The adapter still converts
  nothing; the unit is stated here for the consumer.
- A record the contract refuses (a zero price, say) refuses the frame, and the existing handling of
  a refused frame applies: books are dropped and continuity is reported lost.

## Connection behaviour

The session is a pure state machine; the transport opens the socket and performs what the session
decides.

- A subscribe that is not acknowledged within ten seconds fails the connection. Book traffic arriving
  before the acknowledgement does not promote the connection to live.
- Any inbound frame refreshes an idle timer; forty seconds with no frame, two ping intervals, is a dead
  link.
- Reconnect backoff is exponential with a ceiling of five minutes.
- A gap is reported, and the books are dropped with it.
- Transitions are reported before the actions they cause, so a consumer never sees a state that did
  not happen.

Recorded traffic: `crates/adapters/bybit/fixtures/bybit_linear_orderbook50.jsonl` holds 45 seconds of
the public linear stream across two symbols (2 snapshots, 2769 deltas, 1 subscribe acknowledgement,
2 pongs). The replay tests assert that no frame is refused or skipped, no book ends crossed and the
sequence rule holds, and the market-record conformance check passes over the whole run. Each line's
`at_ns` is the arrival time: in this fixture it is nanoseconds since the recorder connected; fixtures
recorded by the current `examples/record_frames` carry UTC nanoseconds from the platform clock.

## Signed REST

- Signature: `HMAC_SHA256(secret, timestamp_ms || api_key || recv_window || body)`, hex-encoded,
  tested against the RFC 4231 vectors. A separate test changes each input in turn and requires the
  signature to change.
- The credential arrives from the secrets broker and is zeroed on drop.
- Every request carries a deadline. A transport failure classifies as `Unknown`: the request may
  have arrived.
- **A refused credential answers 401 with an empty body** (observed against the live venue,
  2026-08-24). Authentication is checked before matching, so 401, 403 and 429 are rejections: known
  not to have happened. 5xx and a dead transport are `Unknown`.
- A position the adapter cannot read exactly is dropped, never rounded, so reconciliation reports it.
- One TLS provider for the whole adapter: `rustls` with `ring`.

## Instrument currencies (Instruments info, read 2026-11-15)

The linear rows of `GET /v5/market/instruments-info` carry `baseCoin`, `quoteCoin` and
`settleCoin`; the documented example and the live answer of 2026-11-15 for `BTCUSDT` both read
`BTC`, `USDT`, `USDT`. The documentation describes the fields only as base, quote and settle coin.

- A profile's `base_currency` is the asset a quantity is counted in. For a linear perpetual it is
  `baseCoin`: the venue's `qtyStep` and `minOrderQty` are in that coin (`0.001` BTC on `BTCUSDT`).
- Venue assurance reads all three from the row and compares them with the profile without regard
  to letter case. A row without one of them is refused, never read as agreement, and a profile that
  declares no base diverges.

## Execution evidence

Read 2026-11-04 from the pages listed under Sources. The documented response examples are kept as
fixtures in `crates/adapters/bybit/fixtures/documented/` and every parser is tested against them.

| Claim the adapter relies on | Source |
|---|---|
| `GET /v5/order/realtime` lists open orders with `openOnly=0` and up to 500 recent closed orders with `openOnly=1`; `limit` at most 50; paged by `nextPageCursor` | Open orders |
| `GET /v5/order/history`: closed orders of a unified account are read here; `startTime` to `endTime` at most 7 days; `limit` at most 50; a cancelled or rejected order without a fill is kept 24 hours, other closed orders 7 days, after that only orders with fills | Order history |
| `GET /v5/execution/list`: `startTime` to `endTime` at most 7 days; `limit` at most 100; paged by cursor | Executions |
| `GET /v5/position/list`: `limit` at most 200; with `settleCoin` and no `symbol`, only positions of size greater than zero are returned; `size` is always positive and `side` (`Buy`, `Sell`, `""` when empty) carries the direction; `positionIdx` 0 is one-way mode | Positions |
| `settleCoin` and `symbol` are uppercase only | Open orders, Positions |
| Order statuses: open `New`, `PartiallyFilled`, `Untriggered`; closed `Rejected`, `PartiallyFilledCanceled` (spot), `Filled`, `Cancelled` (in derivatives may carry executed quantity), `Triggered` (instantaneous), `Deactivated` | Enums |
| Execution types: `Trade`, `AdlTrade`, `Funding`, `BustTrade`, `Delivery`, `Settle`, `BlockTrade`, `MovePosition`, `FutureSpread`, split settlements, `UNKNOWN` | Enums |
| One execution message may carry several executions of one order; `execId` identifies an execution | Private execution |
| Private stream authentication: `{"op":"auth","args":[key, expires, signature]}`, signature HMAC-SHA256 of `GET/realtime` followed by `expires` (ms), hex | Connect |
| Categorised private topics `order.linear` and `execution.linear`; all-in-one and categorised topics cannot share a subscription request | Private order, Private execution |
| A cancel accepted while the order executes can produce two `Filled` order messages, the second with `cancelType=CancelByUser` and `rejectReason=EC_OrigClOrdIDDoesNotExist` | Private order |

How the adapter reads them:

- `Trade`, `BlockTrade`, `AdlTrade`, `BustTrade` and `Delivery` are fills: each moves a position. An
  execution with an empty `orderLinkId` names no order of this platform and reconciles as external
  (a liquidation or auto-deleveraging closure, for example).
- `Funding` is cash, not quantity: never applied to a position, and recorded as a charge to the
  account under its `execId`, at `execFee` in `feeCurrency`. Positive is paid: a long ETHUSDT
  position at a positive rate read `execFee` `0.00322492` while the transaction log wrote
  `funding` `-0.00322492` for the same `tradeId` (measured 2026-11-19 16:00 UTC). Funding
  received, a negative figure, is not yet seen on a record; the daily loss leaves it out as it
  leaves out a rebate.
- Every other execution type, an undocumented order status, a hedge-mode position, a row of
  another category or a figure finer than its scale is refused by field. A refused row marks the
  evidence incomplete, so reconciliation cannot call it aligned.
- A catalogue symbol absent from the positions read by settle coin is an explicit flat, because the
  venue lists only positions of size greater than zero for that query.
- Every page of a list query is read; a page the venue refuses fails the read rather than reading
  as an empty list. The account read now pages and refuses an unreadable row, where it used to read
  only the first page and drop rows it could not read.

**Open question, flagged for review.** The documentation read on 2026-11-04 shows `execFee` as a
positive figure for a taker execution and does not state its sign for a maker rebate. The adapter
carries the figure with its sign into the platform's fee, where positive is paid and negative a
rebate. That reading of a negative figure is not documented; it must be confirmed on an account
with a maker rebate tier before fee figures are used for capital decisions. `feeCurrency` is empty
in the documented linear example; an empty fee asset means the venue did not state it.

## Account margin (Get Wallet Balance, read 2026-11-20)

`/v5/account/wallet-balance?accountType=UNIFIED`, read with the positions each pass. The margin is
`totalMarginBalance` ("totalWalletBalance + totalPerpUPL", USD) and `totalMaintenanceMargin` (USD).
`totalEquity` is not used: it values every coin, collateral or not (one account read
`totalEquity` 164711.72 against `totalMarginBalance` 99941.31). The venue marks account-wide fields
as not applicable to isolated margin, and an account in isolated margin mode answered them empty:
such an account states no margin. A read that fails leaves the margin unstated; positions are not
withheld for it.

## Private stream (WebSocket connect guide, read 2026-11-16)

A frame on the order or execution topic, an unreadable frame, and every attach after a gap raise
one flag; the node reads the venue evidence over signed REST on its next pass
(`VenueExecution::pushed`). Order states come from that read alone.

Fills are the exception, because the execution history lags the venue. Measured 2026-11-18 on four
market orders: the order status showed `Filled` within 1.7 s, the execution pushed on the stream
arrived within 0.7 s of the send, and `/v5/execution/list` listed the execution 17 to 127 s later,
whether queried by `orderId` or by the settle-coin window the adapter uses. Every pushed row compared
with its listed row agreed on `execId`, `orderId`, side, price, quantity, `execFee`, `feeCurrency`,
`isMaker` and `execTime`. The link therefore keeps each pushed fill for 15 minutes (at most 1000), and
each evidence read adds the ones its history did not list and that executed inside its window. The
same execution listed later is a duplicate in the lifecycle, never a second fill. A lapsed link or a
restart falls back to the history alone; the lifecycle then waits up to 180 s for fills an order's
status already shows (`FILL_LAG_NS`) before it calls the gap a divergence.

| Venue statement | Adapter |
|---|---|
| Hosts: `wss://stream.bybit.com/v5/private`, testnet `wss://stream-testnet.bybit.com/v5/private`, demo trading `wss://stream-demo.bybit.com/v5/private` | Paired with `https://api.bybit.com`, `https://api-testnet.bybit.com` and `https://api-demo.bybit.com`. Regional hosts are not paired: their venues are read on the reconciliation interval alone. Demo trading (Demo trading service, read 2026-11-17) serves private streams only on its host; its public market data is the mainnet stream `wss://stream.bybit.com/v5/public/...`, and order entry over WebSocket is not offered |
| `auth` with an expiry in milliseconds later than the venue clock | Signed on the wall clock with ten seconds of validity, after the socket opens |
| Ping every 20 seconds; the private pong is `{"op":"pong",...}` | Ping every 20 seconds; no frame for 40 seconds lapses the link |
| Do not open more than 500 connections in 5 minutes per domain | Reconnects back off from 3 seconds to 5 minutes, with jitter, and never stop: a lapsed link costs latency only |

The stream runs on its own thread, so the node's pass cadence never starves its heartbeat. Whether
the venue answers a refused `auth` with `success:false`, as the subscription does, is not shown on
the page; any answer other than success, or none within ten seconds, lapses the link.
