# Polymarket

`bijective-venue-polymarket` connects to Polymarket's central limit order book. The kind of
instrument it trades is decided in [decision 0002](../decisions/0002-prediction-market-outcomes.md).

| Capability | State |
|---|---|
| Market stream (`book`, `price_change`, `last_trade_price`) | Built and verified live 2026-11-15: 90 seconds, ten outcomes, 4623 frames, none refused |
| `market_resolved` | Recorded as a `Resolution` fact per held outcome (data class `resolution`). The control engine then marks a won outcome at 1 and a lost one at 0, restamped each pass, and refuses every proposal on it (decision 0002, rule 5) |
| `tick_size_change` | Parsed; not yet recorded |
| Venue assurance (tick size, minimum order size) | Not built |
| Order signing, submission and cancel | Built and tested against a local server; not verified live (no funded account) |
| Venue evidence (orders, trades) and account read (positions) | Built and tested against a local server; not verified live |
| User stream (orders and trades) | Not built |

## Sources

Read 2026-11-15. A claim here is evidence about the date it was read. Every page is available as
Markdown by appending `.md`, so a re-read can diff the documents.

| Source | Document | URL |
|---|---|---|
| Index | Documentation index | `https://docs.polymarket.com/llms.txt` |
| Lifecycle | Order lifecycle | `https://docs.polymarket.com/concepts/order-lifecycle.md` |
| Prices | Prices and order book | `https://docs.polymarket.com/concepts/prices-orderbook.md` |
| Market data | Market details | `https://docs.polymarket.com/market-data/market-details.md` |
| Books | Prices and order books | `https://docs.polymarket.com/market-data/prices-order-books.md` |
| Stream | Real-time data | `https://docs.polymarket.com/market-data/realtime-data.md` |
| Orders | Place orders | `https://docs.polymarket.com/trading/place-orders.md` |
| Manage | Manage orders | `https://docs.polymarket.com/trading/manage-orders.md` |
| User stream | Real-time order updates | `https://docs.polymarket.com/trading/realtime-order-updates.md` |
| Positions | How positions work | `https://docs.polymarket.com/trading/positions/how-positions-work.md` |
| Errors | Error codes | `https://docs.polymarket.com/resources/error-codes.md` |
| Contracts | Contracts | `https://docs.polymarket.com/resources/contracts.md` |
| API | CLOB OpenAPI | `https://docs.polymarket.com/api-spec/clob-openapi.yaml` |
| Auth | Wallets and authentication (read 2026-11-16) | `https://docs.polymarket.com/trading/wallets-auth.md` |
| Activity | Wallet activity (read 2026-11-16) | `https://docs.polymarket.com/trading/wallet-activity.md` |
| Data API | Data API OpenAPI (read 2026-11-16) | `https://data-api.polymarket.com/v2/openapi.json` |
| Limits | Rate limits (read 2026-11-16) | `https://docs.polymarket.com/api-reference/rate-limits.md` |

## What the documents state

- **Every order is a limit order**, signed as EIP-712 typed data and matched off-chain; a matched
  trade settles on Polygon (chain id 137). Types: `GTC`, `GTD`, `FOK`, `FAK`, and post-only.
  Order statuses: `live`, `matched`, `delayed`, `unmatched`. Trade statuses: `MATCHED`, `MINED`,
  `CONFIRMED` (terminal), `RETRYING`, `FAILED` (terminal) (Lifecycle).
- **Prices lie between 0 and 1** in pUSD per share (Prices). Tick sizes are `0.1`, `0.01`,
  `0.005`, `0.0025`, `0.001` and `0.0001` (Orders), so prices are carried at scale 4. Order
  amounts are fixed-point with six decimals (API, `makerAmount`), so quantities are carried at
  scale 6.
- **The tick size can change while a market trades**: the stream publishes `tick_size_change`,
  and an order off the current tick is rejected (Orders, Stream).
- **Outcomes are identified by asset id**: a CTF token id, or a position id on a Protocol V2
  market (Positions). Each binary market has a YES and a NO outcome.
- **The order carries no client identifier.** Its identity is the venue's `orderID`; `salt` and
  `timestamp` are chosen by the signer (Orders, API).
- **The order carries a `builder` field** (bytes32) for integrator attribution (API). The
  platform writes it as zero (decision 0002, rule 8).
- **Signing domain** (Orders): name `Polymarket CTF Exchange`, chain id 137; version `"3"` with
  verifying contract `0xe3333700cA9d93003F00f0F71f8515005F6c00Aa` for a book that returns
  `version: "v2"`; version `"2"` with `0xE111180000d2663C0091e4f400237545B87B996B` for a CTF book
  without negative risk and `0xe2222d279d744050d28e00520010520000310F59` with it. The order type
  is `Order(uint256 salt,address maker,address signer,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint8 side,uint8 signatureType,uint256 timestamp,bytes32 metadata,bytes32 builder)`.
  `expiration` travels in the request body and is not signed.
- **Market stream** (Stream): `wss://ws-subscriptions-clob.polymarket.com/ws/market`; subscribe
  with `{"assets_ids": [...], "type": "market"}`; send the text frame `PING` every 10 seconds and
  the server answers `PONG`. `book` is a snapshot per asset; `price_change` carries changed levels
  with the best bid and ask; `custom_feature_enabled` adds `best_bid_ask`, `new_market` and
  `market_resolved` (with `winning_asset_id`).

## Measured on the live stream (2026-11-15)

Each of these differs from, or is absent from, the documents, and the adapter is built on the
measurement:

- **The opening snapshot is one array** of `book` events, one per subscribed outcome; every
  later event is one object. A `book` also carries `tick_size` and `last_trade_price`.
- **Both sides of a book arrive from the far end**: bids ascending, asks descending. The adapter
  holds both best first.
- **An empty side is stated as a best price of `0`** in a `price_change`.
- **A level a trade consumed can vanish without a change of its own.** The best bid and ask a
  `price_change` states are the authority: a held level better than them is removed and the
  removal recorded. A stated best the held book does not hold cannot be rebuilt; that book is
  dropped and asked for again.
- **Subscribing an outcome already subscribed adds nothing** and the venue answers the text
  `NO NEW ASSETS`; no snapshot follows. A book is asked for again by unsubscribing and
  subscribing the outcome, which sends its snapshot.
- **`price_change` side `BUY` is a bid level and `SELL` an ask level.** The documents do not say;
  every change on the recordings agreed with the venue's stated best prices on that reading.
- **The venue's host resolves to IPv6 first**; a client that waits on an IPv6 path it cannot use
  times out in the handshake. The adapter's connection worked over the host's route on the day
  measured; a node without IPv6 egress is to be checked on its own network.

## Execution (built 2026-11-16)

`rest::RestClient` and `rest::Execution`, opened by the registry with
`VenueCredential::WalletKeyAndApiKey` (wallet key, optional Deposit Wallet address, API key,
secret, passphrase) against `https://clob.polymarket.com` only. What it rests on:

- **Two wallet kinds** (Wallets and authentication, Place orders): a key that holds its own funds
  (`signatureType` 0), and a Deposit Wallet (`signatureType` 3), the smart wallet every
  polymarket.com account deployed since 2026-05-04 uses. For a Deposit Wallet the order's maker
  and signer are the wallet; the key signs the order nested in `TypedDataSign` (name
  `DepositWallet`, version `1`, the wallet as verifying contract, zero salt) under the exchange's
  domain, and the signature is wrapped for ERC-7739 validation. The order hash is the exchange's
  digest of the order itself. Vectors were computed independently with `eth-account`. Requests
  are still made in the key's name (`POLY_ADDRESS`), since the API key is created by the key;
  the wallet is the address for the order, trade and position reads. Proxy and Safe wallets (1
  and 2) are refused.

- **Request authentication** (Place orders, Manage orders): five headers, `POLY_ADDRESS`,
  `POLY_API_KEY`, `POLY_PASSPHRASE`, `POLY_SIGNATURE`, `POLY_TIMESTAMP` (seconds). The signature is
  `urlsafeBase64(HMAC-SHA256(base64Decode(secret), timestamp + METHOD + path + body))`; the query
  string is not signed. A cancel's body is exactly `{"orderID":"<hash>"}`.
- **Signing exchange per outcome** (Place orders, confirmed on a live book 2026-11-16): `GET /book`
  states `version: "v2"` for a protocol V2 book; a CTF book omits it and `neg_risk` selects the
  exchange. Read once per outcome; a book stating neither is refused.
- **Order identity** (decision 0002, rule 7): the salt comes from the client identifier and the
  timestamp is the instant the intent was recorded, so the hash is known before the call. A
  second submit of one identifier is answered `Duplicate` without signing; an answer naming
  another hash is `Unknown`.
- **Restart**: the node hands the adapter every order the record routed here before the first
  evidence read (`VenueExecution::adopt`). The record fixes every signed field but the exchange,
  which the outcome's book chose, so each order is held under the hash it signs to on each of the
  three exchanges, with no request made. The first evidence row naming one of them settles which
  it is and the other two are dropped. An order the venue's evidence has already named is held
  under that hash alone. Until then a cancel for it is refused without a request, and an
  unfinished one costs up to three single-order reads per evidence pass.
- **Evidence**: `GET /data/orders` (live orders), `GET /data/order/{hash}` for each order this
  client placed that the live list no longer holds, until the venue reports it terminal (`404` is
  not read as refused), and `GET /data/trades?maker_address=...&after=...`. As taker the trade row
  is the wallet's order; as maker the wallet's entries in `maker_orders` are, each with its own
  asset, side and amount. A trade not `CONFIRMED` is not a fill: it is taken out of its order's
  matched size and the order stays working (decision 0002, rule 6); evidence holding one is
  incomplete. A `FAILED` trade is refused for an operator.
- **Positions** come from the data service (`GET /v2/positions?user=...&filter_type=TOKENS&filter_amount=0`)
  for the account read only. They are not offered as reconciliation evidence: holdings settle on
  chain after a match, so a comparison in between would read as divergence.
- **Limits**: 600 reads a minute against published limits of 500 per 10 seconds (order and trade
  lists) and 200 per 10 seconds (positions). Five failures open a path for 30 seconds.

Limits of this build, stated so they are not discovered in production:

- **Proxy and Safe wallets** (types 1 and 2) are not built.
- **The wallet should trade for this node only.** A holding or trade outside the node's catalogue
  makes the account read withdraw the venue and the evidence external.
- **Fees** are not carried on fills: a trade states a rate (`fee_rate_bps`), not an amount.

## Open questions, flagged for review

- **Minimum order size: shares or collateral?** Place orders says `min_order_size` is "the
  minimum number of shares the CLOB accepts". Market details annotates `minimumOrderSize` as
  "minimum USDC notional". The two pages disagree; the platform will not declare a floor until an
  order against the venue settles which one holds.
- **Fees.** `fee_rate_bps` appears on trades and a `/fee-rate` endpoint exists; how a fee is
  charged on a fill is to be read before execution is declared.
- **Trade size unit.** The reference's `Trade` example writes `size: "100000000"` while the guide
  writes `"10"` and `OpenOrder` sizes are decimal shares. The adapter reads decimal shares; a
  reading off by 1e6 would exceed the order's matched size and surface as a reconciliation
  finding, never silently.
- **Trade list shape and status names.** The reference pages an object with `TRADE_STATUS_*`
  statuses; the guide shows a bare array with `MATCHED`. Both are read.
- **Deposit Wallet under a session signer.** The CLOB reference says a session signer wraps
  its signature "in a session-signer envelope"; Place orders documents only the account
  signer's ERC-7739 wrap, which is what is built. Whether a scoped session signer needs another
  envelope, and whether the venue accepts `POLY_ADDRESS` as the key while the order's signer is
  the wallet, is settled only by an accepted order.
- **Address case.** `POLY_ADDRESS` is sent in EIP-55 form, the order's `maker` and `signer` in
  lower case. Whether the venue compares them without case is settled only by an accepted order.
- **Dust floor.** `filter_amount=0` is sent to lower the data service's default floor of 0.1
  shares. Whether 0 is accepted or falls back to the default was not observable: no wallet read
  on 2026-11-16 held less than 0.1 shares.
- **`404` on a single order.** Read as "nothing to report", so an unanswered submit stays
  unresolved for an operator rather than being declared never placed.
