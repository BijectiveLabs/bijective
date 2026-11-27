# 0007: Precision per instrument, read from the profile

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-19 |
| Supersedes | The `price_scale` and `quantity_scale` of a venue access and the data engine's `--price-scale` and `--quantity-scale` |

## Context

A price or quantity crosses the platform as an integer at a scale: `2654500` at two places is
26545.00. The profile already states each instrument's scale (`precision.price_scale`,
`precision.quantity_scale`), and admission, valuation and the strategy protocol read it per
instrument. The adapters and the data engine did not: each node configuration gave one price scale
and one quantity scale per venue, and the data engine one per feed. One venue lists instruments of
different precision (read 2026-11-19 on Bybit linear: both write prices to two places, but
BTCUSDT writes quantities to three, a lot of 0.001, and ETHUSDT to two, a lot of 0.01), so a node
could trade only instruments that happened to share a scale, and the configured figure was a second
source of a fact the profile states.

## Decision

1. **The profile is the one source.** Every adapter reads an instrument's scales from the profile's
   catalogue by its venue symbol (`InstrumentScales`). A venue access no longer names a scale, and
   the data engine no longer takes one; both read the profile they are already given. A node
   configuration that still names `price_scale` or `quantity_scale` is refused with this reason,
   so no operator believes a figure that no longer acts.
2. **A record outside the catalogue is read for its presence.** A venue record of an instrument the
   profile does not list (a hand-placed order, a position from another tool) cannot belong to this
   platform's orders. An adapter reads it at twelve places, so it is never refused for its digits,
   and reconciliation judges it as it did before: an external order or position, a divergence.
3. **Where one scale is still needed, mixed precision is refused, not guessed.** The market archive
   writes one scale per venue directory, and an adapter not yet reading scales per instrument takes
   one pair for all its instruments. Each opens only when every instrument of its profile shares a
   scale, and otherwise says which two differ and that one feed or node per precision is the fix.

## Consequences

- One node trades every instrument of a venue's profile, whatever their precision, on adapters
  that read scales per instrument (Bybit first). Hyperliquid, whose assets differ in size decimals,
  Alpaca and Polymarket follow, each when a node needs it.
- Configurations and drills lose two fields per venue and two flags per feed.
- Archiving instruments of different precision through one data engine waits for a per-instrument
  archive scale; until then such a feed runs without `--archive`, or one feed per precision.
