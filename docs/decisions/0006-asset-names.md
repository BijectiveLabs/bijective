# 0006: One asset, many venue names

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-18 |
| Extends | [0001](0001-underlying-scope-level.md): what an underlying is keyed by |

## Context

An instrument is a platform identity on one venue, and the venue's symbol for it is translated by
its adapter: a strategy, a desk and a limit never name a venue symbol. The asset an instrument is
counted in is a different matter. Decision 0001 keys an underlying by the instrument's
`base_currency`, as the venue writes it, and venues write one asset under different names. Read on
2026-11-18: PEPE is `PEPE` on Bybit spot (`PEPEUSDT`), `1000PEPE` on Bybit linear
(`1000PEPEUSDT`) and `kPEPE` on Hyperliquid. Bitcoin is `XBT` on some venues.

Keyed by the venue's name, those positions fall into three underlyings, and a limit declared at
`underlying:pepe` sees the spot holding only. The figure is understated, which is the one direction
a limit must never fail in.

## Decision

1. **An operator names the asset behind a venue's name.** The node configuration may declare
   `assets`: each platform asset, in lower case, with the venue base names that denote it, for
   example `"assets": { "pepe": ["1000pepe", "kpepe"], "btc": ["xbt"] }`. Names compare without
   regard to letter case.
2. **Applied once, where a profile is read.** The node rewrites each instrument's base to its
   asset when it reads a venue profile, for the venues it trades and the markets it reads rates
   from. Admission, valuation and the underlying figures all read the rewritten catalogue, so no
   consumer can apply the table differently. The profile file itself is not changed: it still
   states what the venue states, and venue assurance still verifies it against the venue.
3. **A name denotes one asset.** A name listed under two assets, or listed as an alias while it is
   itself declared as an asset, refuses the configuration.
4. **Money figures need no unit.** One `1000PEPE` contract is a thousand PEPE, and the venue prices
   the contract, so quantity times price is the money held whatever the contract's size. Every
   limit this build declares is in money. A limit in units of the asset would need each name's
   size and is a later decision.
5. **Currencies are not aliases.** A settlement or quote currency is never rewritten: two tokens
   with similar names (USDT and USDT0, USDC and a bridged USDC) are different claims, and one is
   brought into another only at a rate (decision 0005).

## Consequences

- An underlying limit binds one asset across every venue and contract form once its names are
  declared.
- A name the table does not list keeps the venue's spelling, in lower case, as before. An operator
  who trades an asset under two names and declares an underlying limit on it must list both; the
  documentation says so.
- Strategies still see the venue's base name in the instrument list; carrying the asset to them is
  a later change, made when a strategy needs it.
