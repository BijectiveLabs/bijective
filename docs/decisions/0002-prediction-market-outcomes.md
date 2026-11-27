# 0002: Prediction-market outcome instruments

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-15 |
| First venue | Polymarket (`docs/integrations/polymarket.md`) |

## Context

The platform trades spot pairs and linear perpetuals, and refuses dated futures and inverse
contracts because it cannot value or settle them correctly. A prediction market trades a third
thing: a share in one outcome of a question, which pays one unit of collateral if that outcome is
true when the question resolves and nothing otherwise. Polymarket prices a share between 0 and 1
in pUSD, matches orders off-chain and settles them on Polygon. Shares are bought with collateral
and sold only from holdings: there is no short.

Building a venue of this kind proves the concepts carry beyond centralised and decentralised
derivatives venues, so the decisions below are stated generically and Polymarket is the first
case.

## Decision

1. **A new instrument kind, `outcome`.** A share in one outcome of one question. It is tradable.
   Its quantity counts shares, its price is collateral per share and lies in `[0, 1]`, and its
   quote and settlement currency is the collateral. Each outcome is its own instrument; the
   instrument's `base_currency` is the venue's identifier for that outcome.
2. **Valued like a spot holding.** Exposure is shares times the mark, gross and net alike,
   because a share can only be held long. The most a held position can lose is its value at the
   mark, and the most a buy can lose is what it pays.
3. **A price outside `(0, 1)` is refused at admission**, never sent: no share trades at or beyond
   its payout bounds.
4. **A sell must be covered.** An order that sells an outcome, or a spot asset, beyond what is
   held at that venue is refused at admission. The venue would reject it too, but a refusal the
   platform makes is recorded with its reason before anything leaves. Execution on a venue of
   this kind is not enabled until this check exists.
5. **Resolution is a market fact.** When a question resolves, the data adapter records which
   outcome won. From then on the winning outcome is marked at 1 and every other at 0, so a
   position still held is valued at what it redeems for, not left without a mark (which would
   halt the node). The instrument stops admitting orders. Redemption is a movement of holdings
   outside the order path; until a redemption is read as such, it is reconciled as an external
   change, never assumed.
6. **A fill is final when the venue says so.** A trade the venue matched but has not settled
   on-chain is not yet a fill: the order stays one that may execute, so every exposure bound
   counts it, and the position read from the venue stays the record. A settlement that fails
   leaves the order as the venue reports it.
7. **The order's identity is fixed before it is signed.** Where a venue identifies an order by
   the hash of its signed content and carries no client identifier, the adapter derives every
   field it chooses (salt, timestamp) from the platform's client identifier and the intent, so a
   retry signs the same order, the venue sees the same identity, and a duplicate is recognisable.
8. **No attribution by default.** Where an order carries a builder or referral field, it is
   written as the empty value, as section 2.12 of the engineering rules requires and the
   `Order attribution` gate enforces.

## Consequences

- Admission learns two refusals: a price outside an outcome's bounds, and an uncovered sell.
- Valuation learns that a resolved outcome has a settled value.
- A tick size that changes while a market trades (Polymarket publishes `tick_size_change`) is
  observed by venue assurance; a profile that no longer matches the venue diverges and trading on
  that instrument is held, as for any other venue.
- Combinatorial and negative-risk groupings (several outcomes of one event) are not netted
  against each other. Each outcome is valued on its own; any netting is a later decision.

## Alternatives not taken

- **Value an outcome as a dated future.** It has an end, but no margin and a bounded payout;
  treating it as a future would carry the refusal that exists for futures into a kind the
  platform can value exactly.
- **Treat resolution as delisting only.** A position then has no mark and the node halts at the
  moment a winning position is worth most. Recording the result keeps it valued.
