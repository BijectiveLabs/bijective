# 0009: Time in force and post-only

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-19 |
| Extends | The order request, the proposal and the strategy protocol |

## Context

An order carried a side, a quantity, an optional limit price and a reduce-only flag. How long it
rested was each adapter's own choice (Alpaca `gtc` for a limit, Polymarket `GTC`, Bybit the venue's
default `GTC`), so a strategy could not ask for an immediate-or-cancel taker order, a fill-or-kill,
a day order on an equity, an order at the open or close, or a maker order that never takes
liquidity.

Read on 2026-11-19: Bybit V5 Place Order takes `timeInForce` `GTC`, `IOC`, `FOK` and `PostOnly`
("If the order would be filled immediately when submitted, it will be cancelled"), and a market
order always uses `IOC`; it has no expiry field. Alpaca takes `day`, `gtc`, `opg`, `cls`, `ioc` and
`fok` for whole-share equity orders (IOC, FOK, OPG and CLS marked as needing the broker's
enablement), has no post-only and no expiry time.

## Decision

1. **An order states its terms.** `time_in_force` is one of `gtc` (the default, what every limit
   order did before), `ioc`, `fok`, `day`, `at_the_open` and `at_the_close`; `post_only` says the
   order may only rest. A proposal carries them from the strategy to the order, and records written
   before them read as `gtc`, not post-only.
2. **Contradictions are refused at admission**, whatever the venue: a post-only order without a
   limit price, and a post-only order that may not rest (`ioc`, `fok`).
3. **A venue that cannot honour a term refuses the order before it is sent**, and says which term.
   No term is mapped to a near one: an `ioc` sent as `gtc` would rest an order the strategy meant to
   take or cancel. Bybit honours `gtc`, `ioc`, `fok` and post-only with `gtc`; Alpaca honours `gtc`,
   `day`, `ioc`, `fok`, `at_the_open` and `at_the_close`, without post-only; the other venues
   honour `gtc` without post-only until each is read and mapped.
4. **The simulated venue honours what it can and refuses the rest**: `gtc` rests, `ioc` cancels
   what did not fill at once, `fok` fills whole or not at all, post-only is cancelled when it would
   take; `day`, `at_the_open` and `at_the_close` need a session calendar it does not have and are
   refused.
5. **Expiry at a time, conditional triggers, icebergs and linked orders are later decisions.** No
   venue read here offers an expiry time; triggers (stop, take-profit) and contingency (one cancels
   the other, one triggers the other) are held and released by the platform, behind the risk gate,
   and decided on their own record.

## Consequences

- A strategy can take liquidity without leaving a residue, post maker-only quotes on Bybit, and
  trade an equity's open or close on Alpaca.
- Paper runs reproduce IOC, FOK and post-only outcomes, so a strategy promoted to live meets the
  same answers.
- Each further venue states its terms when its mapping is read from the venue's documentation.
