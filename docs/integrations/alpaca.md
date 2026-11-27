# Alpaca

`bijective-venue-alpaca` connects to US equities through Alpaca's Trading API. The asset class and
its rules: [decision 0004](../decisions/0004-multi-asset-equities.md).

| Capability | State |
|---|---|
| Order submission and cancel | Built: submit, a reused identifier, an unknown identifier and cancel answered as read here (observed 2026-11-17). A stop order exercised through a node 2026-11-23 |
| Venue evidence (orders, fills, positions) | Built: open and finished orders read back by client identifier; fills against the documented shape |
| Account read (positions) | Built |
| Market data stream (quotes, trades) | Built: authenticates, subscribes and keeps the link with pings; read as described below |
| Session gate | Built: an entry is sent only while the venue's clock states the core session |
| Trade updates stream | Built: prompts an evidence read when the venue reports an order event |

## Sources

Read 2026-11-17. A claim here is evidence about the date it was read; the scheduled venue check is
what confirms it today. Every page is listed in `https://docs.alpaca.markets/llms.txt`.

| Source | Document | URL |
|---|---|---|
| Authentication | Authentication | `https://docs.alpaca.markets/docs/authentication` |
| Account activities | Account activities, read 2026-11-18 | `https://docs.alpaca.markets/docs/account-activities` |
| Regulatory fees | Regulatory fees, read 2026-11-18 | `https://docs.alpaca.markets/docs/regulatory-fees` |
| Orders guide | Orders at Alpaca | `https://docs.alpaca.markets/docs/orders-at-alpaca` |
| Place an order | Create an order | `https://docs.alpaca.markets/reference/postorder` |
| Order by client id | Get order by client order id | `https://docs.alpaca.markets/reference/getorderbyclientorderid` |
| Order by id | Get order by id | `https://docs.alpaca.markets/us/reference/getorderbyorderid-1.md` |
| List orders | Get all orders | `https://docs.alpaca.markets/us/reference/getallorders-1.md` |
| Cancel | Delete order by id | `https://docs.alpaca.markets/us/reference/deleteorderbyorderid-1.md` |
| Activities | Account activities | `https://docs.alpaca.markets/us/reference/getaccountactivitiesbyactivitytype-1.md` |
| Positions | All open positions | `https://docs.alpaca.markets/reference/getallopenpositions` |
| Account | Get account | `https://docs.alpaca.markets/us/reference/getaccount-1.md` |
| Fractional | Fractional trading | `https://docs.alpaca.markets/us/docs/fractional-trading.md` |
| Paper | Paper trading | `https://docs.alpaca.markets/docs/paper-trading` |
| Trade stream | WebSocket streaming | `https://docs.alpaca.markets/us/docs/websocket-streaming.md` |
| Market data | Streaming market data | `https://docs.alpaca.markets/us/docs/streaming-market-data.md` |
| Stock stream | Real-time stock data | `https://docs.alpaca.markets/us/docs/real-time-stock-pricing-data.md` |
| Quotes reference | Latest quotes | `https://docs.alpaca.markets/us/reference/stocklatestquotes-1.md` |
| Clock | Market clock | `https://docs.alpaca.markets/us/reference/clock-1.md` |

## Execution

- **Hosts** (Authentication): live `https://api.alpaca.markets`, paper
  `https://paper-api.alpaca.markets`. A live key does not work on paper, or the reverse. The
  endpoint is the host alone: every path the adapter sends starts with `/v2`.
- **Credential**: a key id and a secret key, sent as `APCA-API-KEY-ID` and `APCA-API-SECRET-KEY`
  on every request. The keystore holds them under the names the venue access gives
  (`key_name`, `secret_name`).
- **Stops** (decision 0012; Orders at Alpaca, read 2026-11-20): `stop_limit` with `stop_price` and
  `limit_price`, or `stop` with `stop_price` only when the order reduces, since an elected stop
  executes at the market. `gtc` or `day` only; another time in force is refused unsent.
  Exercised through a node 2026-11-23: a reduce-only sell `stop` was accepted and later cancelled
  by a flatten. A buy `stop_limit` sent while that sell stop rested on the same symbol was refused with
  403, code `40310000`, "potential wash trade detected. use complex orders" (not in the documents
  read), and reads as a rejection. The message points to the venue's own complex orders; whether
  every opposite-side order on a symbol with a resting order is refused is not stated.
- **Place** (`POST /v2/orders`): `symbol`, `side`, `qty`, `client_order_id` (at most 128
  characters), and for a limit order `type: limit`, `time_in_force`, `limit_price`. Amounts
  are strings. A market order is sent only when reduce-only. `time_in_force` follows the order's
  terms (decision 0009; Orders at Alpaca, read 2026-11-19): `gtc` for a limit and `day` for a
  market order by default, `day`, `ioc`, `fok`, `opg` (at the open) and `cls` (at the close) when
  asked. The venue marks IOC, FOK, OPG and CLS as enabled on request for whole shares, and allows
  only DAY for fractional orders, which this adapter does not place. Post-only does not exist for
  equities and is refused unsent. No other field
  is written: `extended_hours` stays false and no partner field exists on this API.
- **Price grid** (Orders at Alpaca): two decimals at or above one dollar, four below. A finer limit
  price is refused before it is sent. The price scale must be at least 4; fills and average entry
  prices finer than the scale are refused as rows, so a scale of 9 is advised.
- **Answers**: 200 with the order is accepted only when its `client_order_id` is this order's; 401
  is a refused credential; 403 (documented as insufficient buying power or shares, and seen for a
  request without a key) and other 4xx are rejections carrying the venue's `code` and `message`;
  429 is a rate limit; 5xx and a dead transport are unknown.
- **A reused client identifier** is refused with 422, code `40010001`, "client_order_id must be
  unique" (observed 2026-11-17; not in the documents read), and reads as `Duplicate`: the venue
  already holds the order. Within one process a second submit is answered `Duplicate` without a
  request.
- **An unknown client identifier** on `GET /v2/orders:by_client_order_id` answers 404 (observed
  2026-11-17), read as "no such order".
- **Cancel**: `DELETE /v2/orders/{order_id}`; 204 asks the venue to cancel, 422 says the order is
  no longer cancelable. The venue cancels by its own identifier; one not yet seen is read first by
  `GET /v2/orders:by_client_order_id`.
- **Evidence**:
  - `GET /v2/orders?status=open&limit=500&direction=asc`. A full page may hide more and marks the
    evidence incomplete.
  - Every order this client placed or took back from the record that the open list no longer
    holds, by `GET /v2/orders:by_client_order_id`, until the venue reports it finished.
  - `GET /v2/account/activities/FILL?after=...&direction=asc&page_size=100`, paged by `page_token`
    (the last activity's `id`). The activity `id` is the fill identity. A fill names only the
    venue's order; one not yet read is linked by `GET /v2/orders/{order_id}` once.
  - `GET /v2/positions`, offered as evidence: the account's position moves with each fill.
- **Statuses**: `new`, `accepted`, `pending_new`, `accepted_for_bidding`, `partially_filled`,
  `pending_cancel`, `pending_replace`, `done_for_day`, `stopped`, `suspended` are working;
  `filled`, `canceled`, `expired`, `rejected` are terminal. `held` (meaning not stated),
  `replaced` and `calculated` are refused for an operator.
- **Restart**: the venue carries the client identifier, so orders taken back from the record are
  found by it without any other state.
- **Limits**: the Trading API states no request limit on the pages read. Reads are held to 100 a
  minute, the figure its examples show. Five failures open a path for 30 seconds.

- **Session gate** (decision 0004, rule 6): before an order that is not reduce-only, the adapter
  reads `GET /v3/clock?markets=NYSE` and sends it only when `phase` is `core`. The phase is held
  until its `phase_until`, and at most a minute. A clock that cannot be read holds the entry back.
  An exit is never held. Observed 2026-11-17: `phase` `closed` before 04:00 ET, with
  `next_market_open` 09:30 ET.
- **Request limits observed** 2026-11-17 in `X-RateLimit-Limit`: 1000 a minute on the Trading API,
  200 on the data API. The gate's 100 reads a minute stays inside both.

## Account margin (Get account, read 2026-11-20)

`/v2/account`, read with the positions each pass: `equity` ("Cash + long_market_value +
short_market_value") and `maintenance_margin` ("Maintenance margin requirement (continuously
updated value)"), in `currency`. The venue assesses margin calls at the end of the day; the figure
read is the one it updates continuously. A read that fails leaves the margin unstated; positions
are not withheld for it.

## Trade updates stream

- **Host**: `wss://api.alpaca.markets/stream`, paired with `https://api.alpaca.markets`, and
  `wss://paper-api.alpaca.markets/stream` with `https://paper-api.alpaca.markets`. Any other
  endpoint has none and is read on the evidence cadence.
- **Link**: `{"action":"auth","key":...,"secret":...}`, answered on the `authorization` stream
  (`authorized` or `unauthorized`); then `listen` for `trade_updates`, answered on `listening`.
  Frames may be text or binary JSON. A WebSocket ping every 30 seconds, lapse after 90 seconds with
  nothing back; an `error` action or an unauthorized answer lapses it; reconnects back off to five
  minutes and never stop.
- **A trigger only**: every `trade_updates` event, an unreadable frame and every attach ask the node
  to read the venue's evidence on its next pass. Order states and fills come from the evidence
  alone, so a lost event costs latency, never a fill.

## Market data

- **Stream**: `wss://stream.data.alpaca.markets/v2/iex` (the exchange's own feed, the one usable
  without a data subscription), or `v2/sip` with one. `wss://stream.data.alpaca.markets/v2/test`
  with symbol `FAKEPACA` answers at any hour.
- **Credential**: the stream authenticates with the account's key id and secret key, within ten
  seconds of connecting. The data engine reads them from the keystore, never from configuration:
  `--keystore <path> --credential <init credential> --key-name <name> --secret-name <name>`, all
  four or none. A venue whose stream needs no account is refused a credential.
- **One connection per account** on most subscriptions: a second data engine on the same account
  is refused with error 406. Run one data engine per account.
- **Link**: `auth`, then `subscribe` to `quotes` and `trades` for the symbols. The venue states no
  heartbeat and a closed market sends nothing for hours, so a WebSocket ping goes every 30 seconds
  and the link lapses after 90 seconds with nothing back, pongs included. A venue error (402 auth
  failed, 406 connection limit, 409 insufficient subscription) lapses it. Reconnects back off from
  one second to five minutes and never stop.
- **Records**: a quote states the whole top of book, so it is a snapshot of one level a side, not
  numbered; a side with price or size zero is empty. A trade carries the venue's trade id and no
  aggressor. Prices arrive as JSON numbers and are read from their decimal text; one finer than the
  price scale refuses the frame.
- **Rates**: the crypto stream `wss://stream.data.alpaca.markets/v1beta3/crypto/us` carries
  USDT/USD and USDC/USD in the same message shapes, behind the same key, and was held open beside
  the stock stream on one account (2026-11-18). It is read for valuation rates only
  ([decision 0005](../decisions/0005-reporting-currency.md)): a second data engine with
  `--stream-name <name>` and a profile listing the pairs as `spot`, at a price scale of at least
  six (USDT/USD was quoted to six decimals). Nothing is traded on it. Quotes arrive only on change,
  40 to 50 seconds apart on 2026-11-18, and a side is sometimes stated empty.
- **Sizes are shares.** The quotes reference states sizes in shares since 3 November 2025 (round
  lots before); the stream page still says round lots. They are read as shares. Compared during the
  session of 2026-11-17 (AAPL, IEX): the stream and `GET /v2/stocks/{symbol}/quotes/latest` gave
  the same sizes at the same quote (40, 80, 120), so the stream carries the reference's unit.
- **Halts** (`statuses`) are not subscribed; a halted symbol stops quoting and the freshness bound
  stops entries on it.

Limits of this build, stated so they are not discovered in production:

- **Whole shares, long only** (decision 0004). Fractional holdings refuse the position read.
- **The account should trade for this node only.** A position or fill the node did not place reads
  as external and reconciliation halts.
- **Fees** are not carried on fills. The venue accrues regulatory fees (TAF on sells, CAT on buys
  and sells) during the day and charges each trading day's total at its end, rounded up to the
  cent, as a `FEE` activity with a sub-type; a fee activity names no order (Regulatory fees, read
  2026-11-18; Account activities, read 2026-11-18). A fill's fee is therefore absent, not zero. Each
  evidence read also reads the `FEE` activities created in its window; each is recorded once by
  its identifier as an account charge (`AccountCharged`), and the daily loss counts it on the day
  it is recorded. The reference does not state the sign of a fee's `net_amount`; the account's
  own fees settle it: four `FEE` activities read 2026-11-19 (sub-types `CAT` and `REG`) carry
  -0.01 to -0.03 USD, so a negative figure is paid. A positive figure is left out of the loss and
  counted, so the loss is never understated. Those activities were created at 01:07 and 01:15 UTC
  the next day and dated the trade date, so the loss counts a day's fees on the following day.
- **Order of evidence**: a `FILL` activity was observed listed one read before the order showed it
  filled (2026-11-17). Reconciliation reads that as a stale report and never undoes the fill.
- **Corporate actions** read as divergence and halt until an operator records the adjustment.

## Open questions, flagged for review

- **A short position's `qty`**: whether it carries a sign is not stated; the `side` field decides.
- **Request limits** for the Trading API are not published on the pages read.
- **Trade stream frames** arrived binary on the host read. Whether every host sends binary is not
  stated.
