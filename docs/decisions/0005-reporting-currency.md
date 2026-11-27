# 0005: One reporting currency per node, converted at rates read from declared markets

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-18 |
| Supersedes | The refusal of a second settlement currency in the valuation (`ForeignSettlement`) as the only behaviour |

## Context

The valuation holds a book to one settlement currency. The first position valued fixes it, and a
position settling in any other currency is unvalued, which empties the exposure snapshot and
leaves every limit unjudged. Decision 0004 made the platform multi-asset: an equity settles in
US dollars, a linear perpetual on a centralised venue in USDT, an outcome share or a perpetual on
a decentralised venue in USDC. A node that trades two of them cannot be valued, so it halts on
every pass.

Treating one currency as another at par would be an invented figure. A stablecoin trades near its
peg, not at it: on 2026-11-18 Alpaca quoted USDT/USD at 0.998404 bid and 0.99946 ask. A rate has
to be read, it has to be traceable to a stored entry like every mark, and it can go stale.

## Decision

1. **One reporting currency per node.** The configuration may declare
   `valuation.currency` (for example `usd`). Every money figure the node judges (exposure at every
   level, unrealised profit, the daily loss and the drawdown) is in that currency, and every money
   limit is read in it.
2. **A rate is the mark of a declared market.** `valuation.rates` names, for each other currency,
   one spot instrument whose base is that currency and whose quote is the reporting currency
   (USDT/USD for `usdt` into `usd`). Its mark, the mid of its book, is the rate. The instrument is
   in a configured venue's catalogue, or in the profile of a market stream declared under
   `valuation.markets`, which is read for rates and never traded: a rate pair can sit on another
   endpoint and price scale than the venue's traded instruments, and one stream carries one scale.
3. **Checked before the node trades.** A rate instrument that is not catalogued, is not spot, or
   whose base and quote are not the declared currency and the reporting currency stops the node at
   start, with the reason. Currencies are compared without regard to letter case. A rate from the
   reporting currency into another (an inverted pair) and a rate through a third currency are not
   read in this build.
4. **Missing or stale is unvalued.** An amount in a currency with no declared rate, a rate whose
   market has no mark, or a mark that is not positive leaves the position unvalued, which empties
   the snapshot and refuses orders. Each rate declares `max_age_ns`, with no default, and a rate
   older than it is unvalued in the same way. The bound is the rate's own, not
   `freshness_bound_ns`: a quote stream sends only changes, and on 2026-11-18 the USDT/USD quote
   on Alpaca's crypto stream changed every 40 to 50 seconds, so the five-second bound that suits a
   traded book would halt a node on a rate that had not moved.
5. **Rounding is away from zero**, at eight decimals, as every money figure is, so a converted
   exposure or loss is never smaller than the exact one.
6. **Realised results and fees are converted at the rate of the pass** that measures them, not at
   the rate when they occurred: the daily loss is recomputed each pass, and the platform records
   no rate history.
7. **Without `valuation`** a node keeps one settlement currency as before: the first position
   valued fixes it, and a position in any other currency is unvalued. A single-currency node needs
   no new setting.

## Consequences

- A node trading equities and USDT perpetuals is valued as one book once a USDT/USD market is
  declared and streamed.
- A rate market is one more market the data engine has to record. Its outage halts new orders as a
  mark outage does: the stricter failure, by intent.
- A limit configured before a reporting currency is declared changes meaning when one is declared
  (from the settlement currency to the reporting currency). The operator declares both together.
- A rate history, inverted pairs and cross rates are later decisions, taken when a venue set needs
  them.
