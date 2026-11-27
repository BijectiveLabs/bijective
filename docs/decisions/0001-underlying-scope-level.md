# 0001: A sixth limit level, `underlying`

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-15 |
| Supersedes | The note in `bijective-limits` that a sixth level is a decision record, not a schema change |

## Context

Limits apply at five levels: portfolio, desk, venue, strategy and instrument. Each instrument is a
platform identity on one venue: a node refuses to start when two venues declare one identity, so
that two venues' positions are never merged on a coincidence of names.

That leaves no level that binds one asset across venues. A long BTC perpetual on one venue and a
second long BTC perpetual on another are two instruments and two venues. A portfolio limit sees
them only beside every other asset, and no instrument or venue limit sees both. An operator who
wants "no more than this much BTC, wherever it is held" cannot declare it.

The catalogue now carries `base_currency`, the asset a quantity is counted in, read from the venue
and verified by venue assurance where the venue publishes it (`docs/integrations/`).

## Decision

Add `underlying:<asset>` as a sixth scope level, between strategy and instrument:

```
portfolio -> desk -> venue -> strategy -> underlying -> instrument
```

- **Figures.** Gross and net exposure per underlying, summed over every venue's positions in
  instruments whose `base_currency` is that asset, valued at the same marks and in the same
  settlement currency as the portfolio. The asset is compared without regard to letter case, as
  venue assurance compares it.
- **Fail closed.** One held position whose instrument declares no base produces no underlying
  figure at all: an unknown base may be any asset, so every underlying figure could be understated.
  A declared underlying limit is then unjudged and the node halts. An underlying no venue lists has
  no figure, and its limit is unjudged as well.
- **Kinds.** `gross_exposure` and `net_exposure`, as at every other level but the portfolio-only
  `daily_loss` and `drawdown`.
- **Pre-trade.** An admitted order also draws its spend from its underlying's budget, where the
  instrument declares a base.
- **Safe state.** A breach at underlying spans instruments on several venues, so it halts, as a
  breach at any level wider than an instrument does.
- **Records.** The level is a new pinned discriminant in the limit-state and control layouts, whose
  versions advance, so an older build refuses a record that names it instead of misreading it.

## Consequences

- An operator can bound one asset across venues with one declaration.
- A node whose profiles predate `base_currency` cannot judge an underlying limit until the
  profiles declare it. It halts rather than judge against part of the book.
- Desk and strategy figures are not split by underlying. A limit on one asset for one desk is a
  further level combination, added when something needs it.
- A spot holding and a perpetual on the same asset add into one underlying figure. That is the
  intent: both are exposure to the asset.

## Alternatives not taken

- **Shared instrument identity across venues.** Letting two venues declare one identity would
  merge positions per instrument, but it ties the meaning of an identity to a naming coincidence,
  which the node refuses on purpose, and it cannot join a spot pair to a perpetual.
- **A grouping table in configuration.** An operator could list which instruments make an asset.
  That is authored where the venue already states the answer, and it goes stale silently when a
  venue lists a new instrument.
