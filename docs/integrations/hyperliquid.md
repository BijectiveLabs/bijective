# Hyperliquid

`bijective-venue-hyperliquid` connects to Hyperliquid perpetuals.

| Capability | State |
|---|---|
| Book stream (`l2Book`) | Verified live 2026-11-04: parser, feed and conformance replay on a recording; socket driver tested against a local server |
| Signing | Built; the assembled signature is confirmed only by an accepted live call |
| Account read | Built; not yet verified against the live venue |
| Order submission and cancel | Built; not yet verified against the live venue, and the capability declaration may not claim execution until it is |
| Account stream (`orderUpdates`, `userFills`) | Built: prompts an evidence read when the venue reports a change; tested against a local server, not yet against the venue |
| Venue evidence (orders, fills, funding, positions) | Built; row shapes, signs and ordering read from live answers; the read tested against a local server in those shapes; not yet run against an account this platform trades |

## Sources

Read 2026-08-26 unless stated. A claim here is evidence about the date it was read; the scheduled
venue check is what confirms it today.

| Source | Document | URL |
|---|---|---|
| Docs index | Documentation index (machine-readable) | `https://hyperliquid.gitbook.io/hyperliquid-docs/llms.txt` |
| Tick and lot | Tick and lot size | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size` |
| Nonces | Nonces and API wallets | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/nonces-and-api-wallets` |
| Signing | Signing | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/signing` |
| Fees | Fees | `https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees` |
| Info | Info endpoint, live | `POST https://api.hyperliquid.xyz/info`, body `{"type":"meta"}` |
| Asset ids | Asset IDs | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids` |
| Errors | Error responses | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/error-responses` |
| Subscriptions | WebSocket subscriptions (read 2026-11-06, again 2026-11-16) | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions` |
| Perpetuals metadata | Info endpoint, perpetuals (read 2026-11-15) | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals` |
| Rate limits | Rate limits and user limits | `https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits` |

Every documentation page is available as Markdown by appending `.md`, so a re-verification can diff
the documents instead of re-reading the site.

Sections marked **Observed** describe venue behaviour the adapter is built to handle that the venue
documentation does not state. Each is to be confirmed against the venue before execution is
declared.

## Instrument currencies (Perpetuals metadata, read 2026-11-15)

`{"type":"meta"}` lists the perpetuals of the first perp dex in `universe`, each with `name` (the
documented example and the live answer of 2026-11-15 read `BTC`, `ETH`, ...) and `szDecimals`. The
clearinghouse state reports margin and unrealised profit in USDC.

- A profile's `base_currency` for a perpetual here is its `name`: sizes carry `szDecimals` decimals
  of that coin. Quote and settlement are `usdc`. This is a reading of the documents, flagged for
  confirmation by an accepted live order before execution is declared.
- Builder-deployed perpetuals (another `dex`) are not carried.

## Venue assurance (read 2026-11-15)

`bijective_venue_hyperliquid::observe` reads one `meta` answer and, for the instrument asked,
observes what the answer states:

| Observed | From | As |
|---|---|---|
| Lot | `szDecimals` | `10^-szDecimals` |
| Tick and price rule | `szDecimals` (Tick and lot) | tick `10^-(6 - szDecimals)`, at most 5 significant figures, integers excepted |
| Base | `name` | the coin a size counts |

The collateral is not in the answer (the main perp dex reads `null` in `perpDexs`), so quote and
settlement are left unobserved and not compared. A profile may carry the figures at the
platform's scales: venue assurance compares the tick and lot by value, so `0.10` at scale 2 and
`0.1` at scale 1 are one rule. Minimum notional is stated only in the error table and is not
observed.

## Precision

### Price rule (Tick and lot)

*"Prices can have up to 5 significant figures, but no more than `MAX_DECIMALS - szDecimals` decimal
places where MAX_DECIMALS is 6 for perps and 8 for spot. Integer prices are always allowed,
regardless of the number of significant figures."*

The rule is not a significant-figure cap: `123456` is valid while `12345.6` is not.

### Size decimals are per asset, and the venue rounds (Tick and lot, Info)

*"Sizes are rounded to the szDecimals of that asset."* Read live: `BTC` 5, `ETH` 4, `SOL` 2,
`ATOM` 2. Because the venue rounds, the platform's lot is `10^(quantity_scale - szDecimals)`, so a
size that fits the platform's lot already fits the venue's and nothing is altered in flight.

### The tick is derived (Tick and lot, Errors; Observed)

The error table carries `Tick`, "Price must be divisible by tick size", but no metadata response
carries a tick. Price decimals are `max(0, 6 - szDecimals)` for perps and `max(0, 8 - szDecimals)`
for spot; the tick is `10^(-price_decimals)` and the lot `10^(-szDecimals)`. Round to five
significant figures first, then truncate to the decimal cap, then floor to the tick.

### Minimum notional (Errors; Observed)

`MinTradeNtl` is 10: dollars for perpetuals, quote token for spot. It is carried per instrument in
the profile as `minimum_notional`.

## Accounts and signing

### Querying with the agent wallet returns an empty result (Nonces)

*"A common pitfall is to use the agent wallet which leads to an empty result."* An empty answer is
a success, so it would read as a flat account and pass every exposure limit. The adapter queries the
account address, never the agent address, and the profile carries both. The account address must
also be used for stream subscriptions; if the two disagree, fills are dropped silently (Observed).

### Nonces are a window, not a counter (Nonces)

*"The 100 highest nonces are stored per address. Every new transaction must have nonce larger than
the smallest nonce in this set and also never have been used before."* Nonces must lie within
`(T - 2 days, T + 1 day)` of the block time. A node whose clock drifts outside the window has every
order refused, and at most a hundred actions may be in flight per signer.

### An agent address is single-use (Nonces)

*"Previously signed actions can be replayed once the nonce set is pruned."* A retired agent wallet
is never reused; rotating one is a credential operation.

### Two signing schemes, and a wrong signature reports a missing account (Signing)

L1 actions and user-signed actions are signed differently; msgpack field order matters, trailing
zeroes fail and addresses must be lowercase. A wrong signature recovers a different signer, so the
venue answers "User or API Wallet ... does not exist" or "Must deposit before performing actions".
Authentication is checked before matching, so such an answer means the order did not happen.

### Local verification does not prove a signature (Signing, read 2026-08-30)

The venue recommends using its SDK and warns: *"Believing that the signature must be correct because
calling recover signer locally results in the correct address."* Sign-then-recover can pass while
the signature is wrong. The primitives (Keccak-256, address derivation, msgpack) are tested against
published vectors; the assembled signature is confirmed only by an accepted live call, and the
capability declaration does not claim execution before that.

## Fees (Fees)

*"Fees are based on your rolling 14 day volume"*, one tier per account across perps and spot. The fee
belongs to the operator's account, not to the venue, so no tier is a platform constant.

## Orders

### The asset id is a position in a list (Asset ids)

Perpetuals use the coin's index in the `meta` response; spot uses `10000 + index`; builder-deployed
perps use `100000 + perp_dex_index * 10000 + index_in_meta`. Mainnet and testnet ids differ. A wrong
integer does not fail: it places the order on a different instrument. The mapping comes from the
venue's own `meta`, and the adapter verifies the name at the index before signing. Delisted assets
are retained in place, which suggests the list is append-only; the documentation does not say so,
so the adapter checks rather than relies on it.

### The client order id is 128 bits of hex (Exchange endpoint, read 2026-11-18)

The `c` field of an order and the `cloid` of `cancelByCloid` take "an optional 128 bit hex string",
for example `0x1234567890abcdef1234567890abcdef`. The platform's identifier is not hex, so the
adapter sends the first 16 bytes of its keccak-256 as `0x` and 32 lower-case hex digits; the same
identifier always gives the same cloid. `openOrders` and `frontendOpenOrders` name no cloid in the
documented examples; `orderStatus`, `historicalOrders` and, in live answers, every fill of
`userFillsByTime` carry one. The adapter maps a cloid back to the platform's identifier from the
orders it placed and the record's orders it adopts at start (Venue evidence, below).

### Reduce-only is refused at placement (Errors)

`ReduceOnly`: "Reduce only order would increase position." The venue refuses rather than clamps, so
the `Reducing` safe state is meaningful on this venue.

### A cancel error does not say whether the order filled (Errors)

`MissingOrder`: "Order was never placed, already canceled, or filled." A recognised cancel error is
therefore `Unknown` and goes to reconciliation. Asking for the order's status by id settles it: an
unknown id was never placed; otherwise the order returns with a specific terminal status
(Observed). An order that is no longer open loses its trigger metadata, so anything the platform
needs to know about a protective order is recorded before it terminates (Observed).

### Rejections with no platform name (Errors)

`PositionIncreaseAtOpenInterestCap`, `PositionFlipAtOpenInterestCap`,
`TooAggressiveAtOpenInterestCap`, `OpenInterestIncrease`, `PerpMaxPosition`,
`MarketOrderNoLiquidity` and `InsufficientSpotBalance` are carried as unclassified rejections with
the venue's own code. `Oracle` maps to price out of band and `PerpMargin` to insufficient margin.
The venue also publishes machine-readable terminal statuses that are more specific than its error
text; the adapter keys on the status, not the sentence (Observed).

### A price change is a cancel and a replace (Observed)

A modify produces a new order id for the replacement and a cancellation of the old id, both under
the same client id. The cancellation can arrive before the acceptance, either can arrive before the
response, and the acceptance can be missing entirely. The client order id is the only durable
handle, and a replacement's quantity must be reduced by what already filled.

### Success outside, failure inside (Observed)

An order or cancel request returns HTTP 200 with status `ok` even when every item inside failed. The
per-item outcomes are a positional array whose entries may be objects or bare strings, and two of
the strings carry no order id. A submission is settled from its per-item entry, and an entry with no
order id is an accepted order tracked by client id.

### No market order type (Observed)

A market order is an immediate-or-cancel limit priced away from the book by a slippage allowance,
and it can be rejected outright for absent liquidity. Fill-or-kill, good-till-date, day and
at-the-open are not supported; post-only is a time-in-force value. The slippage allowance is a
platform decision, stated in the platform's cost convention.

### Fills have no identifier and are re-delivered (Observed)

On the account stream, a fill's identity is synthesised from the transaction hash, order id, price,
size, time and position within the transaction. Fills are re-delivered after a reconnect and are
deduplicated. Sizes are always in base units. The fill history read for evidence carries a trade
id, `tid`, which was unique in every fill read on 2026-11-23 (2,000 of 2,000); evidence keys fills
on it.

### Timestamps are not uniformly milliseconds (Observed)

Most times are milliseconds; scheduled-execution history reports seconds.

## Venue evidence (Info endpoint, read 2026-11-23; observed the same day)

Reconciliation reads what the account holds through five `info` requests, all unsigned and keyed
by the account address (never the signer's, which reads empty). Observed rows were read from the
public answers of an active account on the main environment; no order was placed to read them.

| Request | Venue statement | Adapter |
|---|---|---|
| `historicalOrders` | "Returns at most 2000 most recent historical orders" | Read once per pass. Observed: one row per change of an order, newest first (`open`, then `canceled` or `filled`), so the latest change is kept and at one stamp a finished state outranks `open`. A finished order whose latest change is before the window is left out |
| `orderStatus` | `oid` is an order id or a 16-byte hex cloid; an unknown one answers `{"status":"unknownOid"}` | Asked by cloid for each order of the platform's that may still be live and that the history no longer lists, at most 20 a pass; more leaves the evidence incomplete until the next pass |
| `userFillsByTime` | "Returns at most 2000 fills per response and only the 10000 most recent fills are available"; a time range answers "only 500 elements" | Paged from the window's start. Observed: oldest first from `startTime`. Each next page starts at the newest stamp read, the bound being inclusive, and rows repeat once by `tid`. A page under 500 rows ends the read; 25 pages, or 10,000 fills, leave it incomplete |
| `userFunding` | Same time-range rule | Paged the same way |
| `clearinghouseState` | `assetPositions` with a signed `szi` | Only held positions are listed, so an instrument the node trades that is absent is an explicit flat |

How fields are read:

- Status: every value of the documented table maps to working (`open`), filled, canceled (the ten
  `...Canceled` values and `scheduledCancel`) or rejected (`rejected` and the fifteen
  `...Rejected` values). `triggered` and any value not in the table are refused, which keeps the
  pass from `Aligned`: this adapter places no trigger orders, and what a triggered row means for
  the order it became is not stated.
- Quantity: `sz` is what remains (observed: `0.0` on every filled row, `origSz` on an untouched
  canceled one, between the two on a partly filled one), so filled is `origSz - sz`.
- Fees: `fee` includes any builder fee and is negative for a rebate. `crossed` is read as the side
  that took liquidity. Observed: of 2,000 fills, every one with `crossed` false carried a negative
  fee and every one with `crossed` true a positive one.
- Funding: the page does not state the sign of `usdc`. Observed on 85 rows: a long at a positive
  rate and a short at a negative rate always had a negative `usdc`, the opposite pairs a positive
  one. `usdc` is therefore the account's cash change, and the charge, positive when paid, is its
  negation. Funding rows carry an all-zero hash; coin and stamp were unique in every row read and
  identify the charge.

## Limits

### The action budget is earned (Rate limits)

*"The rate limiting logic allows 1 request per 1 USDC traded cumulatively since address inception...
Each address starts with an initial buffer of 10000 requests. When rate limited, an address is
allowed one request every 10 seconds."* Cancels carry a larger allowance,
`min(limit + 100000, limit * 2)`. A strategy that places and cancels without filling spends a budget
it has not earned. REST weight is separate: 1200 per minute per IP, `clearinghouseState` at weight 2
and most other `info` requests at 20. What the venue returns when the earned budget is exhausted is
not documented and is still to be observed.

The REST client reads with a quota of 60 requests a minute, which stays inside the published 1200
weight per minute whichever `info` read is made (most are weight 20). Exchange actions draw on the
earned budget above and are bounded by the control engine's order rate, not by this quota.

### The open-order cap refuses reduce-only orders first (Rate limits)

*"When an order is placed with at least 1000 other open orders by the same user, it will be rejected
if it is reduce-only or a trigger order."* At the cap, the platform cannot enter the `Reducing` safe
state. Whether the control engine should refuse ordinary orders before the cap is an open design
question.

## Market data

### The book stream has no sequence numbers (Observed)

Book payloads carry the instrument, the levels and a millisecond timestamp, with no sequence number,
no update id and no snapshot or delta marker. Every payload is a complete replacement, and a
dropped message cannot be detected. The adapter declares `GapOfUnknownExtent` after every
reconnect and drops its books in every state except connected.

### A subscription can die while the connection stays healthy (Observed)

A reconnect retains no subscription; every subscription is re-sent. An unsubscribe must repeat the
original subscription exactly, including precision options. One stream can go silent while the
socket and sibling streams keep advancing; the remedy is a targeted resubscribe of that stream, not
a reconnect. Subscription errors arrive on a shared channel with no reference to the request, so a
subscription is confirmed by data arriving. A stream that has produced nothing is measured from when
it was requested, not from the link's last message.

A frame on the error channel is reported as a refused frame on the live path and on replay alike:
the feed reads it the same way in both, so a replayed journal marks the same gaps a live run did.

### Verified against the live venue (2026-11-04)

A 75-second recording of `l2Book` for BTC and ETH from `wss://api.hyperliquid.xyz/ws`
(fixture `crates/adapters/hyperliquid/fixtures/hyperliquid_l2book.jsonl`) is replayed by the
`replay_recorded_traffic` test through the parser and the venue conformance suite. Observed:

- Each subscription is acknowledged once, with the subscription echoed back including
  `nSigFigs`, `mantissa` and `fast` as `null`/`false`.
- In the default mode a coin's book arrives about every 5 to 6 seconds, 13 books per coin in
  75 seconds. A stream quiet for less than that is not silent; `STREAM_SILENT_NS` (20 s) leaves room
  for three missed updates.
- Sending a subscribe for a subscription that is already active is answered on the `error` channel
  with `Already subscribed: {...}`. The error carries the subscription text and no request reference.
- A frame whose size has more decimals than the feed's quantity scale is refused by the parser, the
  stream is then never marked seen, and the session re-subscribes every 20 seconds. The venue
  gives each coin its own size decimals (BTC 5, ETH 4 in `meta` on 2026-11-04); the recording uses
  scale 6 for both price and quantity.

The socket driver is also exercised end to end against a local WebSocket server
(`transport_local_server`). That test found that the driver could never connect: a missing-socket
read returned at once, so the connect timer was never reached. Fixed on 2026-11-04.

### Trades and the asset context (read 2026-11-06, observed live the same day)

Each coin is also subscribed to `trades` and `activeAssetCtx`, sent after the book subscriptions and
on every reconnect. They are not tracked for silence: a quiet market has no trades, and the
book alone decides whether a link is alive. A resubscribe of a quiet book does not repeat them,
because a repeated subscription is answered with an error.

- **Trades.** One record per execution, with the venue's `tid` as the trade identity and `time` in
  milliseconds. The page does not define `side`. A minute of live BTC trades set against the book
  before them put `A` at the bid and `B` at the ask far more often than the reverse, so the adapter
  reads `B` as a buyer aggressor and `A` as a seller aggressor. This is a measurement, not a
  statement by the venue, and a letter that is neither is refused.
- **Asset context.** Restated about once a second in full, with every figure a string although the
  page types them as numbers. It carries no time and no next funding time, so the records say
  `NotPublished` for both. A fact is made only for a figure that changed: the funding rate (the
  venue's hourly fraction, kept as written, `0.0000125` is 125 at scale 7), open interest in base
  quantity, the mark price, and the oracle price. The oracle price is the venue's own reference for
  the underlying and is recorded as the index price.
- Liquidations have no public subscription here, so none are recorded for this venue.

## Account stream (WebSocket, Subscriptions, read 2026-11-16)

Like the first venue's private stream, it only prompts a read: a frame on `orderUpdates` or
`userFills` (the opening `isSnapshot` fills included), an unreadable frame, and every attach after a
gap make the node read the venue evidence on its next pass. Fills keep coming from the evidence read.

| Venue statement | Adapter |
|---|---|
| Hosts `wss://api.hyperliquid.xyz/ws`, testnet `wss://api.hyperliquid-testnet.xyz/ws` | Paired with the matching REST host only |
| `{"method":"subscribe","subscription":{"type":"orderUpdates","user":...}}`, the same for `userFills`; no signature | Both sent on every connect for the configured account address. The page warns an agent wallet's address reads empty, so the signer's address is never used |
| Each subscription is answered on channel `subscriptionResponse`, which echoes it | Live after two acknowledgements; an `error` frame or no answer within ten seconds lapses the link |
| Disconnects happen without announcement; missed data is in the snapshot on reconnect | Every attach asks for an evidence read; reconnects back off from 1 second to 5 minutes and never stop |

The page states no heartbeat for these subscriptions; the market stream's ping every 30 seconds and
60-second silence limit are reused.

## Open questions

1. What the venue returns when the earned action budget is exhausted.
2. The venue's definition of the trade `side` letter, which the adapter reads from a measurement.
3. Whether an asset's integer position can move.
4. Whether the integer-price exemption holds in practice.
5. The venue's text for a repeated client order identifier. The adapter reads an answer containing
   both "already" and "cloid" as `Duplicate`; the error-responses page lists no such text, and the
   conformance test uses an answer written to match, so it proves the reading and not the venue.
