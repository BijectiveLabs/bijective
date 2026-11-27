# Integrations

The professional target portfolio is **Binance, Bybit, Hyperliquid, OKX, Polymarket, Bitget,
Kalshi, Alpaca and Interactive Brokers**, across crypto, prediction markets and US equities
([decision 0004](../decisions/0004-multi-asset-equities.md)). The table, as of 2026-11-26,
distinguishes the target from verified support. A target venue is not used for execution until its
adapter evidence and live assurance are complete.

| Venue | Crate | Market data | Account read | Execution |
|---|---|---|---|---|
| Binance | Planned | Not started | Not started | Not started |
| [Bybit](bybit.md) | `bijective-venue-bybit` | Verified live | Verified live | Exercised through a node |
| [Hyperliquid](hyperliquid.md) | `bijective-venue-hyperliquid` | Verified live | Built, not yet verified live | Built, not yet verified live |
| OKX | Planned | Not started | Not started | Not started |
| [Polymarket](polymarket.md) | `bijective-venue-polymarket` | Verified live | Built, not yet verified live | Built, not yet verified live |
| Bitget | Planned | Not started | Not started | Not started |
| Kalshi | Planned | Not started | Not started | Not started |
| [Alpaca](alpaca.md) | `bijective-venue-alpaca` | Verified live | Built | Exercised through a node |
| Interactive Brokers | Planned | Not started | Not started | Not started |

Each page lists the venue documents the adapter was built from and the date each was read. The
`Venue references` gate refuses an adapter without such a page.

Support levels and how an adapter moves between them are described in
[ADAPTERS.md](../../ADAPTERS.md).
