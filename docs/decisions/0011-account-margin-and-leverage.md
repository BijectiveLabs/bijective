# 0011: Account margin, leverage and margin use

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-20 |
| Extends | The account read, the valuation and the limit kinds |

## Context

The platform read each account's positions, never what backs them. `leverage` existed as a limit
kind with no figure, so it could not be declared, and nothing showed how close a venue was to
liquidating a perpetual position.

Read on 2026-11-20: Bybit V5 Get Wallet Balance states, for a unified account, `totalMarginBalance`
("totalWalletBalance + totalPerpUPL", USD) and `totalMaintenanceMargin` (USD), and marks the
account-wide fields as not applicable to isolated margin; a demo account in isolated margin mode
answered them empty. `totalEquity` values every coin, including coins the account has not switched
on as collateral. Alpaca Get account states `equity` ("Cash + long_market_value +
short_market_value") and `maintenance_margin` ("continuously updated value") in the account's
`currency`; it assesses margin calls at the end of the day.

## Decision

1. **The account read carries the account's margin where the venue states it**: equity, maintenance
   margin, their currency and scale. Bybit gives `totalMarginBalance` (collateral the account
   counts, not `totalEquity`) and `totalMaintenanceMargin`; Alpaca gives `equity` and
   `maintenance_margin`. Other venues, and a Bybit account in isolated margin mode, state none.
2. **A margin that cannot be read does not withdraw the venue.** Positions are read as before; the
   margin is left unstated.
3. **Two ratio figures for the whole book**, at 10000 to one, rounded up: `leverage` is the gross
   exposure over the venues' equity, `margin_use` (a new limit kind) is their maintenance margin
   over that equity, where one is the point a venue may liquidate. Equity and margin are converted
   into the book's currency at declared rates (decision 0005), never at par.
4. **All or nothing.** A venue read this pass that states no margin, equity at or below zero, an
   unconverted currency or an unvalued book leaves no figure, so a declared `leverage` or
   `margin_use` limit is unjudged and halts the node.
5. **Declared at portfolio scope only, in their own unit**: `maximum_ratio_scaled` (30000 is three
   times). A ratio kind given `maximum_money_scaled`, or a money kind given `maximum_ratio_scaled`,
   is refused with the field it takes.

## Consequences

- A node trading Bybit perpetuals in USDT and declaring `leverage` values in USD with a USDT/USD
  rate, because the venue states margin in USD.
- An isolated-margin account cannot carry a leverage limit until per-position margin is read.
- The figures are judged each pass and recorded only through a breach or a postcondition; the
  console does not show them yet.
- A drawdown against equity is now possible on these figures and is a later decision.
