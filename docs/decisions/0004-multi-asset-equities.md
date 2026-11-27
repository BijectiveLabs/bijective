# 0004: Multi-asset scope, and US equities through Alpaca first

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-17 |
| First venue | Alpaca (`docs/integrations/alpaca.md`) |

## Context

The platform was built for digital assets: spot pairs and linear perpetuals on centralised and
decentralised venues, and since decision 0002 prediction-market outcome shares. Its safety
machinery (the risk gate, the order lifecycle, reconciliation against venue evidence, the event
store) does not depend on the asset class. Crypto alone is a small part of professional trading,
and the clients this platform serves trade across asset classes.

Two brokers were weighed for US equities. Interactive Brokers reaches far more assets and markets,
but its programmatic access runs through a separate gateway process with its own session. Alpaca
offers US equities through a REST and WebSocket interface shaped like the venues already built,
with a free paper account on the same interface.

## Decision

1. **The platform is multi-asset and multi-venue.** It trades crypto spot and perpetuals on
   centralised and decentralised venues, prediction-market outcomes and US equities, and further
   asset classes as they are decided. Each asset class enters as an instrument kind with its own
   decision record. Nothing above the adapter contract names a venue or an asset class beyond the
   instrument kind.
2. **`equity` is a tradable instrument kind**: a share of a listed company, priced in its quote
   currency and valued like a spot holding. It is **long only** in this build: a sell beyond the
   holding is refused before it is sent, as for spot. Short selling brings borrow, locate and
   short-sale buying power, and is a later decision.
3. **Alpaca is the first equity venue.** Interactive Brokers follows when an asset or market only it
   reaches is needed (section 2.11 of the engineering rules: the shortest path first).
4. **Whole shares only.** The venue trades fractional shares only for the day and never short;
   holding one order kind for every venue keeps the lifecycle the same. The quantity scale is 0.
5. **Limit orders rest until cancelled; a market order only reduces.** Outside the core session an
   order waits for the open, so a market entry would execute on a decision made before it. A market
   order is sent only when it is reduce-only, and lives for the day.
6. **Sessions.** The venue trades in sessions, and orders are sent for the core session only
   (`extended_hours` is never set). Stale market data already stops new entries. A session gate,
   no new entries outside the core session as the venue's clock states it, comes with the market
   data side and is required before execution may be declared for the venue.
7. **Fees** are separate fee activities, typically dated the day after the trade. Fills carry no
   fee in this build, as for Polymarket.
8. **Corporate actions** (splits, symbol changes, dividends paid in shares) change a holding without
   a fill. Reconciliation reads such a change as divergence and halts; an operator records the
   adjustment. Automated handling comes with its own record.
9. **Account rules are the venue's.** Pattern day trading, buying power and trading blocks are
   enforced by the venue; its refusal is a rejection carrying its own words, never retried.
10. **No attribution** (section 2.12): the Trading API order carries no partner field, and none is
    written.

## Consequences

- An operator can trade equities and crypto from one node, under one risk gate and one record.
- The documentation and the public description of the platform name every asset class it trades.
- The free paper account makes Alpaca the first venue whose execution can be exercised against
  the venue's own interface end to end.
- The session gate and the market data side are built (`docs/integrations/alpaca.md`); the trade
  updates stream is the next part of the venue.
