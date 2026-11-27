# 0012: Stop orders and reduce-only proposals

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-20 |
| Extends | The order terms (decision 0009), the proposal, desk admission and `flatten` (decision 0010) |

## Context

An order was a limit or a market order. A position had no protection resting at the venue, so a
node that stopped left it unprotected, and the venues' own disconnect protection is not generally
available. A strategy could not ask for an order that only reduces, so an exit it placed was
withdrawn whenever the node stopped admitting new exposure.

Read on 2026-11-20: Bybit V5 Place Order turns an order with `triggerPrice` into a conditional
order, `triggerDirection` 1 when the price rises to it and 2 when it falls, `triggerBy` the last,
mark or index price, and lists it as `Untriggered` until it triggers; reduce-only applies "if this
order is triggered"; at most 10 conditional orders per symbol. Alpaca takes `stop` (`stop_price`)
and `stop_limit` (`stop_price` and `limit_price`), good till cancelled or for the day. Linked
orders (Alpaca bracket, OCO and OTO; Bybit take-profit and stop-loss attached to an order) create
legs whose client identifiers the venue assigns.

## Decision

1. **A stop is an order term.** `stop_price_scaled`, at the instrument's price scale: a buy stop
   triggers when the price rises to it, a sell stop when it falls to it. With a limit price it is
   a stop-limit, without one a stop-market, bounded by its cap like a market order.
2. **Native only.** Bybit sends a conditional order triggered on the last traded price; Alpaca
   sends `stop_limit`, or `stop` only when the order reduces (an elected stop executes at the
   market, which an entry may not), good till cancelled or for the day. A venue that cannot hold a
   stop refuses the order before it is sent. The simulated venue holds a stop until its best ask
   (a buy) or best bid (a sell) reaches it, the top of book standing in for the last traded price,
   then places the order it carries.
3. **Admission.** A stop is checked against the price increment and the outcome bounds as a price
   is. A stop that is not positive, or that is post-only, is refused. A stop the mark has already
   passed (a buy stop at or below the mark, a sell stop at or above it) is refused
   (`StopAlreadyCrossed`), and a stop-limit's price is collared around its stop, where it becomes a
   limit order. Without a fresh mark a stop is refused.
4. **A proposal may be reduce-only.** It is judged against the record like every reduce-only order
   and draws no budget. A halted or reducing node leaves it working where the venue enforces
   reduce-only, so a protective stop survives a halt, and it rests at the venue if the node stops.
5. **`flatten` orders every open order cancelled**, stops and exits included, and stays in force
   until a resume: each pass closes what the record still holds that working orders do not. On a
   venue that enforces reduce-only an order ordered cancelled is not counted toward flat, so the
   whole position is closed at once; elsewhere it is counted until the venue confirms the cancel.
   A flatten order refused once is not tried again in the same process.
6. **Linked orders are a later decision**, because their legs carry identifiers the venue
   assigns, which reconciliation would read as orders placed outside the platform. Expiry at a
   time and one-updates-the-other are emulated by the platform in that later decision.

## Consequences

- A strategy can place a protective stop that outlives the node and its halts.
- Paper runs trigger stops on the top of book, which may trigger earlier or later than a venue
  triggering on the last trade.
- Records written before this read with no stop and as not reduce-only (proposal layout 6,
  control layout 11).
