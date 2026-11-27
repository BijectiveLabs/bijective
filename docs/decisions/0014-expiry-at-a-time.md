# 0014: Expiry at a time, emulated by the node

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-21 |
| Extends | The order terms (decision 0009), the sweep |

## Context

A strategy could keep an order working until it cancelled it or the session ended, but not until
a time it named. Neither venue that holds stops for the platform holds an expiry: Bybit V5 Place Order
takes `GTC`, `IOC`, `FOK` and `PostOnly` (read 2026-11-19), and Alpaca takes `day`, `gtc`, `opg`, `cls`, `ioc` and
`fok`. Decision 0012 left expiry to a later decision, emulated by the platform.

## Decision

1. **An expiry is an order term.** `expire_at_ns`, UTC nanoseconds. The venue holds the order as
   it would without one; the node cancels it.
2. **Admission.** An expiry needs a time in force that lets the order rest, or the proposal is
   refused (`ExpiryCannotRest`). A proposal whose expiry is at or before the node's clock when it
   is read is refused before admission.
3. **The sweep cancels it.** Each pass withdraws every working order whose expiry is at or before
   the pass's instant, whatever the trading state, as it withdraws an order an operator ordered
   cancelled. The order stays pending until the venue's evidence says it stopped working.
4. **Judged on the node's clock**, the same clock that admitted the order.

## Consequences

- An order may work up to one pass past its expiry, plus the cancel's round trip. A fill in that
  window is a real fill and is recorded as one.
- A node that is stopped at the expiry cancels the order on its first pass after it starts.
- The record reads an expired order as cancelled, not expired: the node, not the venue, ended it.
- Records written before this read with no expiry (proposal layout 8, control layout 13).