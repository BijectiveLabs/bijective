# Adapters

## Support levels

| Level | Meaning |
|---|---|
| Built | The code exists and passes its tests against recorded traffic or a local server |
| Verified live | The capability was run against the live venue and its answers read as the integration page states |
| Exercised through a node | Orders were placed, answered, reconciled and cancelled by a running node against the venue |

An adapter's state never claims more than the evidence supports. A check that needs the live
venue and has not run keeps the capability at the lower level. Whether a node may place orders on a
venue is declared in the venue's profile (support depth `full` or `data-only`), never inferred from
whether the adapter compiles.

## Current adapters

As of 2026-11-26. Each integration page states the evidence and its date.

| Venue | Crate | Market data | Account read | Execution |
|---|---|---|---|---|
| [Bybit](docs/integrations/bybit.md) | `bijective-venue-bybit` | Verified live | Verified live | Exercised through a node |
| [Alpaca](docs/integrations/alpaca.md) | `bijective-venue-alpaca` | Verified live | Built | Exercised through a node |
| [Hyperliquid](docs/integrations/hyperliquid.md) | `bijective-venue-hyperliquid` | Verified live | Built | Built |
| [Polymarket](docs/integrations/polymarket.md) | `bijective-venue-polymarket` | Verified live | Built | Built |
| [Simulated](docs/concepts/simulated_venue.md) | `bijective-venue-simulated` | Not applicable | Not applicable | Built: the venue paper runs and backtests place orders on |

The venue portfolio and what is planned are in [docs/integrations](docs/integrations/index.md).

## Adding a venue

An adapter is one crate under `crates/adapters/<venue>`, an integration page citing the venue's
documents with the date each was read, recorded traffic fixtures, and a registration in
`bijective-venue-registry`. The steps are in
[Adapters](docs/concepts/adapters.md#adding-a-venue).
